#!/usr/bin/env python3
"""Checks the changelog's fragments, or merges them into CHANGELOG.md at a release.

A change that users see adds a fragment to `changelog.d/` rather than editing CHANGELOG.md, so that
two changes made at once never meet in one file. A fragment is named
`<section>-<name>.md`, its section one of SECTIONS, and holds the section's entries, each a list's
item; `changelog.d/README.md` says how to write one.

The merge never loses a line of CHANGELOG.md. It refuses, changing nothing, a CHANGELOG.md without
a `## [Unreleased]` heading, or whose [Unreleased] holds text outside its `###` sections, which it
could not place; the check refuses them too, so that a release never finds them. It writes
CHANGELOG.md through a temporary file and a rename, then deletes the fragments.

    scripts/changelog.py           check every fragment's name and form, and CHANGELOG.md's
                                   [Unreleased]
    scripts/changelog.py --merge   move every fragment into CHANGELOG.md's [Unreleased], by section
"""

import os
import re
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
# Keep a Changelog's sections, and the project's own last one, in the order a release shows them.
SECTIONS = {
    "added": "Added",
    "changed": "Changed",
    "deprecated": "Deprecated",
    "removed": "Removed",
    "fixed": "Fixed",
    "security": "Security",
    "known-limitations": "Known limitations",
}
NAME = re.compile(r"^(" + "|".join(SECTIONS) + r")-[a-z0-9]+(-[a-z0-9]+)*\.md$")
UNRELEASED = "## [Unreleased]\n"
SECTION = re.compile(r"^### ([^\n]+)\n(.*?)(?=^### |\Z)", re.MULTILINE | re.DOTALL)


class Refused(Exception):
    """Why CHANGELOG.md cannot be merged into: the merge then changes nothing."""


def fragments(directory, problems):
    """Each fragment of `directory`: its section, its path and its entries."""
    found = []
    for path in sorted(directory.glob("*.md")):
        if path.name == "README.md":
            continue
        name = NAME.match(path.name)
        try:
            text = path.read_text(encoding="utf-8").strip()
        except (OSError, UnicodeDecodeError) as error:
            problems.append(f"{path.name}: cannot be read: {error}")
            continue
        if not name:
            problems.append(f"{path.name}: not named <section>-<name>.md, its section one of {', '.join(SECTIONS)}")
        elif not text.startswith("- "):
            problems.append(f"{path.name}: its entries are a list, each item starting with `- `")
        else:
            found.append((name.group(1), path, text))
    return found


def unreleased(text):
    """Where [Unreleased]'s body starts and ends in `text`; refuses one it could not merge into."""
    if text.startswith(UNRELEASED):
        start = len(UNRELEASED)
    else:
        heading = text.find("\n" + UNRELEASED)
        if heading == -1:
            raise Refused("CHANGELOG.md has no `## [Unreleased]` heading to merge into: add one above the releases")
        start = heading + 1 + len(UNRELEASED)
    following = text.find("\n## ", start)
    end = len(text) if following == -1 else following + 1
    first_section = re.search(r"^### ", text[start:end], re.MULTILINE)
    outside = text[start:end] if first_section is None else text[start : start + first_section.start()]
    if outside.strip():
        line = outside.strip().splitlines()[0]
        raise Refused(f"CHANGELOG.md's [Unreleased] holds text outside its `###` sections, which a merge could not place: {line!r}")
    return start, end


def merge(text, found):
    """`text`, CHANGELOG.md, with the fragments' entries at the end of their sections of
    [Unreleased]; refuses rather than lose any of its lines."""
    start, end = unreleased(text)
    body = text[start:end] if text[start:end].endswith("\n") else text[start:end] + "\n"
    # [Unreleased]'s sections, two of one name joined, in the order of SECTIONS.
    bodies = {}
    for heading, section in SECTION.findall(body):
        bodies[heading] = (bodies.get(heading, "").rstrip() + "\n\n" + section.strip()).strip()
    for section, _, entries in found:
        heading = SECTIONS[section]
        bodies[heading] = (bodies.get(heading, "") + "\n" + entries).strip()
    order = list(SECTIONS.values()) + sorted(set(bodies) - set(SECTIONS.values()))
    sections = "".join(f"\n### {heading}\n\n{bodies[heading]}\n" for heading in order if heading in bodies)
    following = text[end:]
    merged = text[:start] + sections + ("\n" + following if following else "")
    kept = {line.rstrip() for line in merged.splitlines()}
    lost = [line for line in text.splitlines() if line.strip() and line.rstrip() not in kept]
    if lost:
        raise Refused(f"the merge would lose a line of CHANGELOG.md, {lost[0]!r}: it was left as it was")
    return merged


def write_atomically(path, text):
    """Writes `text` to `path` through a temporary file beside it, flushed to the disk, and a rename:
    `path` is never left half written. Kept in each script of the documents, which runs alone."""
    descriptor, temporary = tempfile.mkstemp(dir=path.parent, prefix=f".{path.name}.", suffix=".partial")
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as file:
            file.write(text)
            file.flush()
            os.fsync(file.fileno())
        os.chmod(temporary, path.stat().st_mode & 0o777 if path.exists() else 0o644)
        os.replace(temporary, path)
    except BaseException:
        Path(temporary).unlink(missing_ok=True)
        raise


def run(root, merge_them):
    """Checks `root`'s fragments and CHANGELOG.md and, with `merge_them`, merges the fragments in;
    gives the problems found, empty when all is well. Changes nothing when it finds one."""
    problems = []
    found = fragments(root / "changelog.d", problems)
    changelog = root / "CHANGELOG.md"
    try:
        text = changelog.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as error:
        return problems + [f"CHANGELOG.md: cannot be read: {error}"]
    merged = None
    try:
        if merge_them:
            merged = merge(text, found)
        else:
            unreleased(text)
    except Refused as refusal:
        problems.append(str(refusal))
    if problems or not merge_them:
        return problems
    try:
        write_atomically(changelog, merged)
    except OSError as error:
        return [f"CHANGELOG.md could not be written, and was left as it was: {error}"]
    for _, path, _ in found:
        try:
            path.unlink()
        except OSError as error:
            problems.append(f"CHANGELOG.md holds {path.name}'s entries, but {path.name} could not be deleted: {error}; delete it by hand, or a merge adds them again")
    return problems


def main():
    if sys.argv[1:] not in ([], ["--merge"]):
        raise SystemExit("usage: scripts/changelog.py [--merge]")
    problems = run(ROOT, merge_them=sys.argv[1:] == ["--merge"])
    for problem in problems:
        print(problem, file=sys.stderr)
    sys.exit(1 if problems else 0)


if __name__ == "__main__":
    main()
