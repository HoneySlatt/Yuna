"""Tests of scripts/changelog.py, on Python's standard library alone: its merge keeps every entry
CHANGELOG.md held, and refuses, changing nothing, what it could not place.

    python3 -I -m unittest discover -s scripts -p 'test_*.py'
"""

import importlib.util
import shutil
import tempfile
import unittest
from pathlib import Path
from unittest import mock

SCRIPTS = Path(__file__).resolve().parent
ROOT = SCRIPTS.parent


def load(name, file):
    """The script `file` as a module, loaded by its path: the scripts are no package, and
    `python3 -I` puts none of their folders on the path."""
    spec = importlib.util.spec_from_file_location(name, SCRIPTS / file)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


changelog = load("changelog", "changelog.py")


def entries(text):
    """Each list item of `text`, its continuation lines with it."""
    found, current = [], None
    for line in text.splitlines():
        if line.startswith("- "):
            current = [line]
            found.append(current)
        elif current is not None and line.startswith("  "):
            current.append(line)
        else:
            current = None
    return ["\n".join(lines) for lines in found]


class Merge(unittest.TestCase):
    def setUp(self):
        self.root = Path(tempfile.mkdtemp(prefix="yuna-changelog-test-"))
        self.addCleanup(shutil.rmtree, self.root)
        (self.root / "changelog.d").mkdir()
        shutil.copy(ROOT / "CHANGELOG.md", self.root / "CHANGELOG.md")
        shutil.copy(ROOT / "changelog.d" / "README.md", self.root / "changelog.d" / "README.md")

    def changelog(self):
        return (self.root / "CHANGELOG.md").read_text(encoding="utf-8")

    def fragment(self, name, text):
        (self.root / "changelog.d" / name).write_text(text, encoding="utf-8")

    def left(self):
        return sorted(path.name for path in (self.root / "changelog.d").iterdir())

    def test_a_merge_into_todays_changelog_keeps_every_old_entry(self):
        old = self.changelog()
        self.assertGreater(len(entries(old)), 5, "today's CHANGELOG.md holds its entries as a list")
        self.fragment("fixed-a-crash-at-start.md", "- A crash at start no longer happens.\n")
        self.fragment("added-a-new-thing.md", "- A new thing, with a line\n  that goes on.\n")
        self.assertEqual(changelog.run(self.root, merge_them=True), [])
        new = self.changelog()
        for entry in entries(old):
            self.assertIn(entry, new)
        self.assertIn("### Fixed\n\n- A crash at start no longer happens.\n", new)
        # A new entry goes at the end of its section, after the old ones.
        added = new[new.index("### Added") : new.index("### Fixed")]
        self.assertTrue(added.rstrip().endswith("- A new thing, with a line\n  that goes on."), added)
        self.assertEqual(self.left(), ["README.md"])
        # Merged again, with no fragment, it changes nothing.
        self.assertEqual(changelog.run(self.root, merge_them=True), [])
        self.assertEqual(self.changelog(), new)
        self.assertEqual(changelog.run(self.root, merge_them=False), [])

    def test_text_outside_the_sections_is_refused_and_nothing_changes(self):
        text = "# Changelog\n\n## [Unreleased]\n\nA paragraph of its own.\n\n### Added\n\n- One.\n"
        (self.root / "CHANGELOG.md").write_text(text, encoding="utf-8")
        self.fragment("added-two.md", "- Two.\n")
        problems = changelog.run(self.root, merge_them=True)
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("outside its `###` sections", problems[0])
        self.assertIn("A paragraph of its own.", problems[0])
        self.assertEqual(self.changelog(), text)
        self.assertEqual(self.left(), ["README.md", "added-two.md"])
        # The check finds it before a release does.
        self.assertEqual(changelog.run(self.root, merge_them=False), problems)

    def test_entries_before_any_section_are_refused(self):
        # As CHANGELOG.md was before its entries were given their section.
        text = "# Changelog\n\n## [Unreleased]\n\n- An entry of no section.\n"
        (self.root / "CHANGELOG.md").write_text(text, encoding="utf-8")
        self.fragment("fixed-x.md", "- X.\n")
        problems = changelog.run(self.root, merge_them=True)
        self.assertIn("An entry of no section.", problems[0])
        self.assertEqual(self.changelog(), text)

    def test_a_changelog_without_unreleased_is_refused_and_nothing_changes(self):
        text = "# Changelog\n\n## [0.1] - 2027-01-01\n\n### Added\n\n- One.\n"
        (self.root / "CHANGELOG.md").write_text(text, encoding="utf-8")
        self.fragment("added-two.md", "- Two.\n")
        problems = changelog.run(self.root, merge_them=True)
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("no `## [Unreleased]` heading", problems[0])
        self.assertEqual(self.changelog(), text)
        self.assertEqual(self.left(), ["README.md", "added-two.md"])

    def test_releases_below_unreleased_are_kept_whole(self):
        text = (
            "# Changelog\n\n## [Unreleased]\n\n### Fixed\n\n- Three.\n\n"
            "## [0.1] - 2027-01-01\n\nA release's own words.\n\n### Added\n\n- One.\n"
        )
        (self.root / "CHANGELOG.md").write_text(text, encoding="utf-8")
        self.fragment("added-two.md", "- Two.\n")
        self.assertEqual(changelog.run(self.root, merge_them=True), [])
        new = self.changelog()
        self.assertTrue(new.endswith("## [0.1] - 2027-01-01\n\nA release's own words.\n\n### Added\n\n- One.\n"), new)
        unreleased = new[: new.index("## [0.1]")]
        self.assertLess(unreleased.index("### Added\n\n- Two."), unreleased.index("### Fixed\n\n- Three."))

    def test_a_write_that_fails_leaves_the_changelog_and_the_fragments(self):
        old = self.changelog()
        self.fragment("added-two.md", "- Two.\n")
        with mock.patch.object(changelog.os, "replace", side_effect=OSError("the disk is full")):
            problems = changelog.run(self.root, merge_them=True)
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("the disk is full", problems[0])
        self.assertEqual(self.changelog(), old)
        self.assertEqual(self.left(), ["README.md", "added-two.md"])
        self.assertEqual([path.name for path in self.root.glob(".*partial")], [])

    def test_a_malformed_fragment_is_named_and_nothing_merged(self):
        old = self.changelog()
        self.fragment("added-two.md", "- Two.\n")
        self.fragment("notes.md", "- Loose.\n")
        (self.root / "changelog.d" / "fixed-bytes.md").write_bytes(b"- \xff\n")
        problems = changelog.run(self.root, merge_them=True)
        self.assertEqual(len(problems), 2, problems)
        self.assertTrue(any(problem.startswith("fixed-bytes.md: cannot be read") for problem in problems), problems)
        self.assertTrue(any(problem.startswith("notes.md: not named") for problem in problems), problems)
        self.assertEqual(self.changelog(), old)


if __name__ == "__main__":
    unittest.main()
