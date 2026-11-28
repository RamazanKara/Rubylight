"""Standalone unit tests for Nightjar's declarative settings.toml catalog and generate-settings.py."""

import importlib.util
import pathlib
import shutil
import tempfile
import tomllib
import unittest

SCRIPTS_DIR = pathlib.Path(__file__).resolve().parent
CLIENT_ROOT = SCRIPTS_DIR.parent


def _load_generator():
    spec = importlib.util.spec_from_file_location("generate_settings", SCRIPTS_DIR / "generate-settings.py")
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


GEN = _load_generator()


class NightjarSettingsCatalogTest(unittest.TestCase):
    def test_catalog_parses_with_stdlib_tomllib_and_valid_cross_refs(self):
        raw = (CLIENT_ROOT / "protocol/policy/settings.toml").read_text(encoding="utf-8")
        parsed = tomllib.loads(raw)
        for section in ("constants", "android", "retired", "renamed", "android_const", "android_setting", "desktop_setting"):
            self.assertIn(section, parsed)
        cat = GEN.load_catalog(CLIENT_ROOT)
        self.assertGreaterEqual(len(cat["android_const"]), 230)
        self.assertGreaterEqual(len(cat["android_setting"]), 180)
        self.assertGreaterEqual(len(cat["desktop_setting"]), 140)
        self.assertGreaterEqual(len(cat["retired"]), 20)
        self.assertGreaterEqual(len(cat["renamed"]), 1)

    def test_all_generated_targets_are_current_lf_and_headed(self):
        stale = GEN.run(check=True, root=CLIENT_ROOT)
        self.assertEqual(stale, [], f"stale Nightjar settings targets: {stale}")
        for rel in GEN.ALL_TARGETS:
            raw = (CLIENT_ROOT / rel).read_bytes()
            self.assertNotIn(b"\r\n", raw, f"{rel} has CRLF line endings")
            text = raw.decode("utf-8")
            if rel.endswith(".rs"):
                self.assertTrue(text.startswith("//! "), f"{rel} missing //! module header")
            if rel.endswith(".java"):
                self.assertIn("/**\n", text.split("public class", 1)[0], f"{rel} missing Javadoc header")

    def test_check_detects_staleness_without_mutating_and_regen_fixes_it(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            # Copy settings.toml, generated targets, and Android/policy inputs used by docs/settings.md.
            copy_files = [
                "protocol/policy/settings.toml",
                "protocol/policy/src/video_pacing.rs",
                "protocol/policy/src/quality.rs",
                "protocol/policy/src/recovery.rs",
                "app/src/main/res/xml/preferences.xml",
                "app/src/main/res/xml/preferences_root.xml",
                "app/src/main/java/com/limelight/preferences/StreamSettings.java",
                "app/src/main/java/com/limelight/preferences/SeekBarPreference.java",
                "app/src/main/java/com/limelight/preferences/StreamPreset.java",
                "app/src/main/java/com/limelight/preferences/PreferenceMigrations.java",
                *GEN.ALL_TARGETS,
            ]
            for val_xml in (CLIENT_ROOT / "app/src/main/res/values").glob("*.xml"):
                copy_files.append(str(val_xml.relative_to(CLIENT_ROOT)).replace("\\", "/"))
            for rel in copy_files:
                dst = root / rel
                dst.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(CLIENT_ROOT / rel, dst)

            self.assertEqual(GEN.run(check=True, root=root), [])

            tampered_rs = root / GEN.TARGET_POLICY_RS
            bad_rs = tampered_rs.read_text(encoding="utf-8") + "// drift\n"
            tampered_rs.write_text(bad_rs, encoding="utf-8", newline="\n")

            tampered_java = root / GEN.TARGET_ANDROID_JAVA
            bad_java = tampered_java.read_text(encoding="utf-8") + "// drift\n"
            tampered_java.write_text(bad_java, encoding="utf-8", newline="\n")

            tampered_md = root / GEN.TARGET_SETTINGS_MD
            bad_md = tampered_md.read_text(encoding="utf-8").replace("## Presets", "## Stale Presets", 1)
            tampered_md.write_text(bad_md, encoding="utf-8", newline="\n")

            expected_stale = sorted([GEN.TARGET_POLICY_RS, GEN.TARGET_ANDROID_JAVA, GEN.TARGET_SETTINGS_MD])
            self.assertEqual(sorted(GEN.run(check=True, root=root)), expected_stale)
            # --check must not modify files on disk.
            self.assertEqual(tampered_rs.read_text(encoding="utf-8"), bad_rs)
            self.assertEqual(tampered_java.read_text(encoding="utf-8"), bad_java)
            self.assertEqual(tampered_md.read_text(encoding="utf-8"), bad_md)

            # Regenerating fixes only stale targets and restores exact byte parity.
            self.assertEqual(sorted(GEN.run(check=False, root=root)), expected_stale)
            self.assertEqual(GEN.run(check=True, root=root), [])
            for rel in expected_stale:
                self.assertEqual((root / rel).read_bytes(), (CLIENT_ROOT / rel).read_bytes(), rel)


if __name__ == "__main__":
    unittest.main()
