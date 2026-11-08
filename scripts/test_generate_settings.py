from pathlib import Path
import runpy
import unittest
from unittest.mock import patch


generate = runpy.run_path(str(Path(__file__).with_name("generate-settings.py")))["generate"]
read_text = Path.read_text


class SettingsReferenceTest(unittest.TestCase):
    def test_bitrate_steps_follow_runtime_logarithmic_control(self):
        text = generate()
        row = next(line for line in text.splitlines() if "`seekbar_bitrate_kbps`" in line)
        self.assertIn("0.5 Mbps below 10 Mbps, 1 Mbps below 100 Mbps, "
                      "5 Mbps below 300 Mbps, 10 Mbps from 300 Mbps", row)
        self.assertNotIn("buttons 1 Mbps", row)

    def test_runtime_default_drift_is_rejected(self):
        def changed(path, *args, **kwargs):
            text = read_text(path, *args, **kwargs)
            if path.name == "PreferenceConfiguration.java":
                text = text.replace("DEFAULT_MULTI_CONTROLLER = true", "DEFAULT_MULTI_CONTROLLER = false")
            return text

        with patch.object(Path, "read_text", changed):
            with self.assertRaisesRegex(ValueError, "XML and runtime defaults differ for checkbox_multi_controller"):
                generate()

    def test_slider_threshold_changes_are_regenerated(self):
        def changed(path, *args, **kwargs):
            text = read_text(path, *args, **kwargs)
            if path.name == "SeekBarPreference.java":
                text = text.replace("value < 10000)", "value < 20000)")
            return text

        with patch.object(Path, "read_text", changed):
            self.assertIn("0.5 Mbps below 20 Mbps", generate())


if __name__ == "__main__":
    unittest.main()
