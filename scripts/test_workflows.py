"""Tests of the workflows' path filters, on Python's standard library alone: the jobs that run real
applications in Yuna beside CI, SlimeEngine's (.github/workflows/slimeengine.yml) and Opium's
(.github/workflows/opium.yml), each run on every push that changes a file its test builds or
reads. Each names what it leaves out (`paths-ignore`), so that a file added to Yuna runs it until
someone says it cannot matter; this test checks that nothing it needs is left out.

    python3 -I -m unittest discover -s scripts -p 'test_*.py'
"""

import re
import subprocess
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
WORKFLOWS = ROOT / ".github/workflows"

# What every job builds and reads apart from the crates: the workspace's manifests and lock, the
# shell it runs in, the toolchain, and the tests' shared helpers.
NEEDED_BY_EVERY_JOB = [
    "Cargo.toml",
    "Cargo.lock",
    "flake.nix",
    "flake.lock",
    "rust-toolchain.toml",
    "crates/yuna/tests/common/",
]

# Each job's workflow, and what it builds and reads beside those: the job itself, and the test it
# runs with its files.
JOBS = {
    "slimeengine.yml": [
        ".github/workflows/slimeengine.yml",
        "crates/yuna/tests/applications.rs",
        "crates/yuna/tests/applications/",
    ],
    "opium.yml": [
        ".github/workflows/opium.yml",
        "crates/yuna/tests/opium.rs",
    ],
}


def tracked_files():
    listed = subprocess.run(["git", "ls-files", "-z"], cwd=ROOT, capture_output=True, check=True)
    return [name for name in listed.stdout.decode().split("\0") if name]


def path_dependencies(crate):
    """The folders, from the repository's root, of the crates `crate` depends on by path, its
    development dependencies included, and theirs: `cargo test` builds them all."""
    found, waiting = set(), [crate]
    while waiting:
        folder = waiting.pop()
        if folder in found:
            continue
        found.add(folder)
        manifest = (ROOT / folder / "Cargo.toml").read_text(encoding="utf-8")
        for relative in re.findall(r'path\s*=\s*"([^"]+)"', manifest):
            waiting.append((ROOT / folder / relative).resolve().relative_to(ROOT).as_posix())
    return found


def needed_files(files, outside_the_crates):
    """Every tracked file a job's test builds or reads: the code of `crates/yuna` and of the
    crates it depends on by path, and the files outside them, `outside_the_crates`, listed above.
    Each crate's own tests, examples and documents are needed only for the test of `crates/yuna`
    the job runs, which its list names."""
    crates = path_dependencies("crates/yuna")
    needed = []
    for name in files:
        if any(name == entry or (entry.endswith("/") and name.startswith(entry))
               for entry in outside_the_crates):
            needed.append(name)
            continue
        for crate in crates:
            inside = name[len(crate) + 1:] if name.startswith(crate + "/") else None
            if inside is not None and (inside == "Cargo.toml" or inside.startswith("src/")):
                needed.append(name)
    return needed


def push_filter(workflow_text):
    """A job's push filter: ("paths" or "paths-ignore", its patterns)."""
    kind, patterns, inside = None, [], False
    for line in workflow_text.splitlines():
        stripped = line.strip()
        if stripped in ("paths:", "paths-ignore:"):
            kind, inside = stripped[:-1], True
            continue
        if inside:
            item = re.fullmatch(r'-\s*"([^"]+)"', stripped)
            if item:
                patterns.append(item.group(1))
            elif stripped and not stripped.startswith("#"):
                inside = False
    return kind, patterns


def glob_regex(pattern):
    """GitHub's filter pattern as a regular expression: `**` any characters, `*` any but `/`."""
    parts = re.split(r"(\*\*|\*|\?)", pattern)
    translated = {"**": ".*", "*": "[^/]*", "?": "[^/]"}
    return re.compile("".join(translated.get(part, re.escape(part)) for part in parts))


def runs_on(kind, patterns, name):
    matched = any(glob_regex(pattern).fullmatch(name) for pattern in patterns)
    return matched if kind == "paths" else not matched


class ApplicationJobs(unittest.TestCase):
    def test_every_file_a_job_needs_runs_it(self):
        files = tracked_files()
        for workflow, outside_the_crates in JOBS.items():
            with self.subTest(workflow=workflow):
                kind, patterns = push_filter((WORKFLOWS / workflow).read_text(encoding="utf-8"))
                self.assertEqual(kind, "paths-ignore",
                                 "the job names what it leaves out, not what it takes")
                self.assertTrue(patterns, "the job's push filter is empty")
                needed = needed_files(files, NEEDED_BY_EVERY_JOB + outside_the_crates)
                missed = [name for name in needed if not runs_on(kind, patterns, name)]
                self.assertEqual(missed, [], "files the job needs that do not run it")

    def test_a_new_file_runs_each_job(self):
        for workflow in JOBS:
            kind, patterns = push_filter((WORKFLOWS / workflow).read_text(encoding="utf-8"))
            for name in ["crates/yuna/src/new_module.rs", "crates/new-crate/src/lib.rs", "build.rs"]:
                with self.subTest(workflow=workflow, name=name):
                    self.assertTrue(runs_on(kind, patterns, name))

    def test_the_patterns_follow_githubs_rules(self):
        self.assertTrue(glob_regex("**.md").fullmatch("docs/adr/0001-what-yuna-is.md"))
        self.assertTrue(glob_regex("docs/**").fullmatch("docs/adr/README.md"))
        self.assertFalse(glob_regex("scripts/*.py").fullmatch("scripts/a/b.py"))
        self.assertFalse(glob_regex("docs/**").fullmatch("crates/yuna/src/docs.rs"))


if __name__ == "__main__":
    unittest.main()
