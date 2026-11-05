"""Release notes must not combine adjacent preview and stable entries."""
import unittest

from release_notes import release_notes


class ReleaseNotesTests(unittest.TestCase):
    changelog = '''# Changelog

## 0.26.0-preview.2 - 2026-11-05

### Added

- New preview features.

## 0.26.0 - 2026-11-03

- Original release features.

## 0.25.1 Preview 5 - 2026-10-26

- Older features.
'''

    def test_exact_preview_contains_only_its_own_changes(self):
        self.assertEqual(release_notes(self.changelog, 'v0.26.0-preview.2'),
                         '### Added\n\n- New preview features.\n')

    def test_legacy_preview_falls_back_to_exact_base_version(self):
        self.assertEqual(release_notes(self.changelog, 'v0.26.0-preview.1'),
                         '- Original release features.\n')
        self.assertEqual(release_notes(self.changelog, 'v0.26.0'),
                         '- Original release features.\n')

    def test_missing_empty_duplicate_and_invalid_versions_fail(self):
        cases = (
            ('## 0.26.1\n- Different release.\n', 'v0.26.0-preview.2'),
            ('## 0.26.0-preview.2\n\n## 0.26.0\n- Old release.\n', 'v0.26.0-preview.2'),
            ('## 0.26.0\n- One.\n## 0.26.0\n- Two.\n', 'v0.26.0'),
            (self.changelog, '../../main'),
        )
        for changelog, tag in cases:
            with self.subTest(tag=tag, changelog=changelog), self.assertRaises(ValueError):
                release_notes(changelog, tag)


if __name__ == '__main__':
    unittest.main()
