"""Tests of scripts/adr-index.py, on Python's standard library alone: ADR numbers read in links and
plainly, supersessions named on both sides from ADR-0071, malformed ADRs named, never a traceback,
and an index written whole or not at all.

    python3 -I -m unittest discover -s scripts -p 'test_*.py'
"""

import importlib.util
import shutil
import tempfile
import unittest
from pathlib import Path
from unittest import mock

SCRIPTS = Path(__file__).resolve().parent


def load(name, file):
    """The script `file` as a module, loaded by its path: the scripts are no package, and
    `python3 -I` puts none of their folders on the path."""
    spec = importlib.util.spec_from_file_location(name, SCRIPTS / file)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


adr_index = load("adr_index", "adr-index.py")

README = (
    "# Architecture decision records\n\nThe rules.\n\n"
    "| ADR | Title | Status |\n|---|---|---|\n"
    "| [0001](0001-old.md) | An old row | Accepted |\n"
    "\nWhat follows the index.\n"
)


class Index(unittest.TestCase):
    def setUp(self):
        self.directory = Path(tempfile.mkdtemp(prefix="yuna-adr-index-test-"))
        self.addCleanup(shutil.rmtree, self.directory)
        (self.directory / "README.md").write_text(README, encoding="utf-8")
        (self.directory / "0000-template.md").write_text("# NNNN. A title\n", encoding="utf-8")

    def adr(self, number, status, title=None):
        path = self.directory / f"{number}-adr-{number}.md"
        title = title or f"The decision {number}"
        path.write_text(f"# {number}. {title}\n\n- Status: {status}\n- Date: 2026-11-16\n\n## Context\n", encoding="utf-8")
        return path

    def readme(self):
        return (self.directory / "README.md").read_text(encoding="utf-8")

    def rows(self):
        return [line for line in self.readme().splitlines() if line.startswith("| [")]

    def test_numbers_written_plainly_count_as_links_do(self):
        self.adr("0076", "Accepted; context corrected on 2026-11-12: a date is no ADR")
        self.adr("0077", "Superseded by 0078")
        self.adr("0078", "Accepted; supersedes 0077")
        self.adr("0079", "Accepted; supersedes ADR-0076")
        self.adr("0080", "Superseded by [0081](0081-adr-0081.md) for the engine")
        self.adr("0081", "Accepted; supersedes [0080](0080-adr-0080.md)")
        problems = adr_index.run(self.directory, write=True)
        # 0076 is superseded by 0079 on 0079's side alone: from ADR-0071, both sides must say so.
        self.assertEqual(problems[:1], ["0076-adr-0076.md: its status does not say that 0079 supersedes it"])
        self.adr("0076", "Superseded by 0079; context corrected on 2026-11-12")
        self.assertEqual(adr_index.run(self.directory, write=True), [])
        self.assertEqual(
            self.rows(),
            [
                "| [0076](0076-adr-0076.md) | The decision 0076 | Superseded by 0079 |",
                "| [0077](0077-adr-0077.md) | The decision 0077 | Superseded by 0078 |",
                "| [0078](0078-adr-0078.md) | The decision 0078 | Accepted; supersedes 0077 |",
                "| [0079](0079-adr-0079.md) | The decision 0079 | Accepted; supersedes 0076 |",
                "| [0080](0080-adr-0080.md) | The decision 0080 | Superseded by 0081 for the engine |",
                "| [0081](0081-adr-0081.md) | The decision 0081 | Accepted; supersedes 0080 |",
            ],
        )
        # What surrounds the index is kept.
        text = self.readme()
        self.assertTrue(text.startswith("# Architecture decision records\n\nThe rules.\n\n| ADR |"), text)
        self.assertTrue(text.endswith("|\n\nWhat follows the index.\n"), text)
        self.assertEqual(adr_index.run(self.directory, write=False), [])

    def test_a_supersession_named_on_one_side_fails_from_adr_0071_only(self):
        # Before ADR-0071, a supersession named by the newer ADR alone counts, as it was written.
        self.adr("0027", "Accepted")
        self.adr("0042", "Accepted; supersedes [0027](0027-adr-0027.md)")
        # From ADR-0071, both must name it, whichever side forgot.
        self.adr("0071", "Accepted")
        self.adr("0075", "Accepted; supersedes 0071")
        self.adr("0076", "Superseded by 0077")
        self.adr("0077", "Accepted")
        problems = adr_index.run(self.directory, write=False)
        self.assertEqual(
            sorted(problems[:-1]),
            [
                "0071-adr-0071.md: its status does not say that 0075 supersedes it",
                "0077-adr-0077.md: its status does not say that it supersedes 0076",
            ],
        )
        self.assertIn("run scripts/adr-index.py --write", problems[-1])
        # Not written while they stand.
        problems = adr_index.run(self.directory, write=True)
        self.assertIn("README.md was left as it was", problems[-1])
        self.assertEqual(self.readme(), README)

    def test_a_status_naming_an_adr_that_does_not_exist_is_refused(self):
        self.adr("0072", "Accepted; amends 0099")
        self.assertEqual(
            adr_index.run(self.directory, write=False)[0],
            "0072-adr-0072.md: its status names 0099, which does not exist",
        )

    def test_a_malformed_adr_is_named_never_a_traceback(self):
        self.adr("0071", "Accepted")
        self.adr("0072", "(an aside before any state)")
        self.adr("0073", "")
        (self.directory / "0074-no-status.md").write_text("# 0074. No status\n\n## Context\n", encoding="utf-8")
        (self.directory / "0075-wrong-heading.md").write_text("# 0057. Wrong\n\n- Status: Accepted\n- Date: x\n", encoding="utf-8")
        (self.directory / "0076-bytes.md").write_bytes(b"# 0076. \xff\n")
        # An ADR naming a malformed one is not told that it does not exist.
        self.adr("0077", "Accepted; supersedes 0072")
        problems = adr_index.run(self.directory, write=True)
        named = [problem.split(":")[0] for problem in problems[:-1]]
        self.assertEqual(
            named,
            ["0072-adr-0072.md", "0073-adr-0073.md", "0074-no-status.md", "0075-wrong-heading.md", "0076-bytes.md"],
            problems,
        )
        self.assertIn("does not start with its state", problems[0])
        self.assertIn("README.md was left as it was", problems[-1])
        self.assertEqual(self.readme(), README)

    def test_a_readme_without_its_index_is_named(self):
        (self.directory / "README.md").write_text("# ADRs\n\nNo table.\n", encoding="utf-8")
        self.adr("0071", "Accepted")
        self.assertEqual(adr_index.run(self.directory, write=True), [
            "README.md: no index, a table headed `| ADR | Title | Status |`, to write the ADRs into"
        ])
        self.assertEqual(self.readme(), "# ADRs\n\nNo table.\n")

    def test_a_write_that_fails_leaves_the_readme_whole(self):
        self.adr("0071", "Accepted")
        with mock.patch.object(adr_index.os, "replace", side_effect=OSError("the disk is full")):
            problems = adr_index.run(self.directory, write=True)
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("the disk is full", problems[0])
        self.assertEqual(self.readme(), README)
        self.assertEqual([path.name for path in self.directory.glob(".*partial")], [])

    def test_the_last_row_may_end_the_file(self):
        (self.directory / "README.md").write_text(README.split("\nWhat follows")[0].rstrip("\n"), encoding="utf-8")
        self.adr("0071", "Accepted")
        self.assertEqual(adr_index.run(self.directory, write=True), [])
        self.assertEqual(self.rows(), ["| [0071](0071-adr-0071.md) | The decision 0071 | Accepted |"])


if __name__ == "__main__":
    unittest.main()
