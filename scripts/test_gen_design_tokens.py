import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import gen_design_tokens as gen  # noqa: E402


class ColourTest(unittest.TestCase):
    def test_android_writes_alpha_first(self):
        self.assertEqual(gen.android_color("#14161F66"), "#6614161F")

    def test_opaque_colours_stay_six_digits_on_android(self):
        self.assertEqual(gen.android_color("#F2A93B"), "#F2A93B")
        self.assertEqual(gen.android_color("#0A0A0CFF"), "#0A0A0C")

    def test_egui_colours_are_premultiplied(self):
        self.assertEqual(gen.premultiplied("#FFFFFF80"), (128, 128, 128, 128))
        self.assertEqual(gen.premultiplied("#F2A93B"), (242, 169, 59, 255))
        self.assertEqual(gen.premultiplied("#00000000"), (0, 0, 0, 0))

    def test_a_bad_colour_is_refused(self):
        with self.assertRaises(ValueError):
            gen.rgba("#12345")


class KindTest(unittest.TestCase):
    def test_the_name_decides_the_unit(self):
        self.assertEqual(gen.kind("radius.panel", 28), "dp")
        self.assertEqual(gen.kind("glass.chrome.lens.blur", 26), "dp")
        self.assertEqual(gen.kind("glass.chrome.lens.saturate", 1.9), "float")
        self.assertEqual(gen.kind("type.body.size", 16), "sp")
        self.assertEqual(gen.kind("type.body.track", 0), "float")
        self.assertEqual(gen.kind("type.body.weight", 400), "int")
        self.assertEqual(gen.kind("motion.fade.ms", 120), "int")
        self.assertEqual(gen.kind("motion.press.stiffness", 1500), "float")
        self.assertEqual(gen.kind("color.moon", "#F2A93B"), "color")
        self.assertEqual(gen.kind("reduced.spring", "none"), "string")

    def test_names(self):
        self.assertEqual(gen.snake("glass.chrome.lens.rimTop"), "glass_chrome_lens_rim_top")
        self.assertEqual(gen.snake("color.moon"), "color_moon")


class OutputTest(unittest.TestCase):
    def setUp(self):
        self.tokens = gen.load()

    def test_every_token_reaches_every_client(self):
        for key in self.tokens:
            name = gen.snake(key)
            self.assertIn(f'name="nj_{name}"', gen.android(self.tokens))
            self.assertIn(f"pub const {name.upper()}:", gen.rust(self.tokens))
            self.assertIn(f"--nj-{name.replace('_', '-')}:", gen.css(self.tokens))

    def test_the_committed_output_is_current(self):
        for path, text in gen.outputs(self.tokens).items():
            self.assertEqual(path.read_bytes(), text.encode(), f"{path.name} is stale")


if __name__ == "__main__":
    unittest.main()
