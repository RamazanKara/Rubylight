#!/usr/bin/env python3
"""Tests for play_upload.py's release notes and track release."""

import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import play_upload  # noqa: E402


class ReleaseNotesTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def changelog(self, locale, code, text):
        path = self.root / locale / 'changelogs' / f'{code}.txt'
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding='utf-8')

    def test_maps_locales_and_trims(self):
        self.changelog('en-US', 386, '• Fixed things.\n')
        self.changelog('de', 386, '• Dinge repariert.\n')
        self.changelog('de', 385, 'old')
        self.assertEqual(play_upload.release_notes(self.root, 386), [
            {'language': 'de-DE', 'text': '• Dinge repariert.'},
            {'language': 'en-US', 'text': '• Fixed things.'},
        ])

    def test_missing_changelog_gives_no_notes(self):
        self.changelog('en-US', 385, 'x')
        self.assertEqual(play_upload.release_notes(self.root, 386), [])

    def test_rejects_notes_over_limit(self):
        self.changelog('en-US', 386, 'x' * (play_upload.NOTES_LIMIT + 1))
        with self.assertRaises(ValueError):
            play_upload.release_notes(self.root, 386)

    def test_repository_changelogs_fit(self):
        metadata = Path(__file__).resolve().parent.parent / 'fastlane' / 'metadata' / 'android'
        for changelog in metadata.glob('*/changelogs/*.txt'):
            with self.subTest(changelog=changelog):
                self.assertLessEqual(len(changelog.read_text(encoding='utf-8').strip()), play_upload.NOTES_LIMIT)

    def test_track_release(self):
        self.assertEqual(play_upload.track_release(386, '4.12.9', 'completed', []),
                         {'name': '4.12.9', 'versionCodes': ['386'], 'status': 'completed'})
        notes = [{'language': 'en-US', 'text': 'x'}]
        self.assertEqual(play_upload.track_release(386, '4.12.9', 'draft', notes)['releaseNotes'], notes)


if __name__ == '__main__':
    unittest.main()
