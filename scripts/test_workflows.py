"""Tests of the workflows' path filters, on Python's standard library alone: SlimeEngine's job
(.github/workflows/slimeengine.yml) runs on every push that changes a file its test builds or
reads. It names what it leaves out (`paths-ignore`), so that a file added to Yuna runs it until
someone says it cannot matter; this test checks that nothing it needs is left out.

    python3 -I -m unittest discover -s scripts -p 'test_*.py'
"""

import re
import subprocess
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
WORKFLOW = ROOT / ".github/workflows/slimeengine.yml"

# What the job builds and reads apart from the crates: the workspace's manifests and lock, the
# shell it runs in, the toolchain, the job itself, and the test it runs with its helpers.
NEEDED_OUTSIDE_THE_CRATES = [
    "Cargo.toml",
    "Cargo.lock",
    "flake.nix",
    "flake.lock",
    "rust-toolchain.toml",
    ".github/workflows/slimeengine.yml",
    "crates/yuna/tests/applications.rs",
    "crates/yuna/tests/applications/",
    "crates/yuna/tests/common/",
]


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


def needed_files(files):
    """Every tracked file the job's test builds or reads: the code of `crates/yuna` and of the
    crates it depends on by path, and the files outside them listed above. Each crate's own
    tests, examples and documents are needed only for `crates/yuna`'s test of applications,
    which the list names."""
    crates = path_dependencies("crates/yuna")
    needed = []
    for name in files:
        if any(name == entry or (entry.endswith("/") and name.startswith(entry))
               for entry in NEEDED_OUTSIDE_THE_CRATES):
            needed.append(name)
            continue
        for crate in crates:
            inside = name[len(crate) + 1:] if name.startswith(crate + "/") else None
            if inside is not None and (inside == "Cargo.toml" or inside.startswith("src/")):
                needed.append(name)
    return needed


def push_filter(workflow_text):
    """The job's push filter: ("paths" or "paths-ignore", its patterns)."""
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


class SlimeEngineJob(unittest.TestCase):
    def test_every_file_the_job_needs_runs_it(self):
        kind, patterns = push_filter(WORKFLOW.read_text(encoding="utf-8"))
        self.assertEqual(kind, "paths-ignore", "the job names what it leaves out, not what it takes")
        self.assertTrue(patterns, "the job's push filter is empty")
        missed = [name for name in needed_files(tracked_files()) if not runs_on(kind, patterns, name)]
        self.assertEqual(missed, [], "files the job needs that do not run it")

    def test_a_new_file_runs_it(self):
        kind, patterns = push_filter(WORKFLOW.read_text(encoding="utf-8"))
        for name in ["crates/yuna/src/new_module.rs", "crates/new-crate/src/lib.rs", "build.rs"]:
            self.assertTrue(runs_on(kind, patterns, name), name)

    def test_the_patterns_follow_githubs_rules(self):
        self.assertTrue(glob_regex("**.md").fullmatch("docs/adr/0001-what-yuna-is.md"))
        self.assertTrue(glob_regex("docs/**").fullmatch("docs/adr/README.md"))
        self.assertFalse(glob_regex("scripts/*.py").fullmatch("scripts/a/b.py"))
        self.assertFalse(glob_regex("docs/**").fullmatch("crates/yuna/src/docs.rs"))


if __name__ == "__main__":
    unittest.main()
