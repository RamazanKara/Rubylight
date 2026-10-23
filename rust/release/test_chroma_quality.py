# Local measurement tool tests: they need numpy and pillow, which CI does not install.
# Run: python -m unittest discover -s rust/release -p test_chroma_quality.py
import pathlib, shutil, tempfile, unittest
import numpy as np
import chroma_fixtures as fixtures
import chroma_quality as quality


def planes(rng, width, height, maximum):
    return rng.integers(0, maximum + 1, (3, height, width), dtype=np.int32)


class PackedLayout(unittest.TestCase):
    def test_version_one_round_trips_every_sample(self):
        rng = np.random.default_rng(1)
        for width, height, maximum in [(66, 70, 255), (130, 72, 1023), (64, 64, 255)]:
            source = planes(rng, width, height, maximum)
            packed = quality.pack(source)
            self.assertEqual(packed['y'].shape[0], 2 * quality.padded(width, height)[1])
            self.assertTrue(np.array_equal(quality.unpack(packed, width, height), source))

    def test_version_two_costs_at_most_one_code(self):
        rng = np.random.default_rng(2)
        for maximum in (255, 1023):
            source = planes(rng, 66, 70, maximum)
            packed = quality.pack(source, layered=True, maximum=maximum)
            restored = quality.unpack(packed, 66, 70, layered=True, maximum=maximum).astype(np.int32)
            self.assertTrue(np.array_equal(restored[0], source[0]))
            self.assertLessEqual(int(np.abs(restored - source).max()), 1)

    def test_packed_picture_keeps_odd_columns_in_the_luma_plane(self):
        source = np.zeros((3, 8, 64), np.int32)
        source[1, :, 1::2] = 200
        packed = quality.pack(source)
        self.assertEqual(int(packed['y'][8:, :32].min()), 200)
        self.assertEqual(int(packed['u'].max()), 0)


class Reconstruction(unittest.TestCase):
    def test_reference_conversion_round_trips_within_two_codes(self):
        rng = np.random.default_rng(3)
        rgb = rng.integers(0, 256, (16, 16, 3), dtype=np.uint8)
        for matrix in (0, 1):
            for bits in (8, 10):
                back = quality.rgb_from_ycbcr(quality.reference_ycbcr(rgb, matrix, bits), matrix, bits)
                self.assertLessEqual(int(np.abs(back.astype(int) - rgb).max()), 6 if bits == 8 else 3)

    def test_upsampling_keeps_flat_chroma_and_cannot_keep_one_pixel_stripes(self):
        flat = np.full((4, 8), 77, np.uint16)
        self.assertTrue((quality.upsample_chroma(flat, 16, 8) == 77).all())
        rgb = np.zeros((16, 16, 3), np.uint8)
        rgb[:, ::2] = (255, 0, 0)
        rgb[:, 1::2] = (0, 0, 255)
        truth = quality.reference_ycbcr(rgb)
        half = truth[::2, ::2, 1]
        cb = quality.upsample_chroma(half, 16, 16)
        self.assertGreater(float(np.abs(cb.astype(int) - truth[..., 1]).max()), 20)
        self.assertEqual(quality.psnr(truth[..., 1], truth[..., 1], 255), float('inf'))

    def test_summary_splits_the_phases(self):
        rows = [dict(m=float(i)) for i in range(90)]
        result = quality.summarize(rows)
        self.assertEqual(result['first_m'], 0.)
        self.assertEqual(result['settled_m'], 89.)
        self.assertAlmostEqual(result['scroll_m'], 44.5)
        self.assertAlmostEqual(result['all_m'], 44.5)

    def test_picked_frames_keep_their_phases(self):
        chosen = quality.picks(90)
        self.assertEqual(chosen[:3], [0, 29, 30])
        self.assertEqual(chosen[-1], 89)
        self.assertEqual(len(chosen), 13)
        rows = [dict(m=float(i), frame=i) for i in chosen]
        result = quality.summarize(rows)
        self.assertEqual(result['first_m'], 0.)
        self.assertEqual(result['settled_m'], 89.)
        self.assertAlmostEqual(result['scroll_m'], sum(range(30, 60, 3)) / 10)
        self.assertEqual(quality.picks(1), [0])


class Fixtures(unittest.TestCase):
    def test_scroll_schedule(self):
        self.assertEqual([fixtures.scroll_offset(f) for f in (0, 29, 30, 31, 59, 60, 89)], [0, 0, 6, 12, 180, 180, 180])

    @unittest.skipUnless((fixtures.FONT_DIRECTORY / 'consola.ttf').exists(), 'needs the Windows fonts')
    def test_frames_are_deterministic_and_scroll_changes_them(self):
        for kind in fixtures.KINDS:
            first = dict(fixtures.frames(kind, 1280, 720, 32))
            fixtures.COVERAGE.clear()
            second = dict(fixtures.frames(kind, 1280, 720, 32))
            self.assertEqual(first, second)
            self.assertEqual(first[0], first[29])
            self.assertNotEqual(first[29], first[31])


if __name__ == '__main__':
    unittest.main()
