#!/usr/bin/env python3
"""Checks the changelog's fragments, or merges them into CHANGELOG.md at a release.

A change that users see adds a fragment to `changelog.d/` rather than editing CHANGELOG.md, so that
two changes made at once never meet in one file. A fragment is named
`<section>-<name>.md`, its section one of SECTIONS, and holds the section's entries, each a list's
item; `changelog.d/README.md` says how to write one.

    scripts/changelog.py           check every fragment's name and form
    scripts/changelog.py --merge   move every fragment into CHANGELOG.md's [Unreleased], by section
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FRAGMENTS = ROOT / "changelog.d"
CHANGELOG = ROOT / "CHANGELOG.md"
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


def fragments(problems):
    found = []
    for path in sorted(FRAGMENTS.glob("*.md")):
        if path.name == "README.md":
            continue
        name = NAME.match(path.name)
        text = path.read_text(encoding="utf-8").strip()
        if not name:
            problems.append(f"{path.name}: not named <section>-<name>.md, its section one of {', '.join(SECTIONS)}")
        elif not text.startswith("- "):
            problems.append(f"{path.name}: its entries are a list, each item starting with `- `")
        else:
            found.append((name.group(1), path, text))
    return found


def merge(found):
    """CHANGELOG.md with the fragments' entries at the end of their sections of [Unreleased]."""
    text = CHANGELOG.read_text(encoding="utf-8")
    start = text.index("## [Unreleased]\n") + len("## [Unreleased]\n")
    following = text.find("\n## [", start)
    end = len(text) if following == -1 else following + 1
    # [Unreleased]'s sections, two of one name joined, in the order of SECTIONS.
    bodies = {}
    for heading, body in re.findall(r"^### ([^\n]+)\n(.*?)(?=^### |\Z)", text[start:end], re.MULTILINE | re.DOTALL):
        bodies[heading] = (bodies.get(heading, "").rstrip() + "\n\n" + body.strip()).strip()
    for section, _, entries in found:
        heading = SECTIONS[section]
        bodies[heading] = (bodies.get(heading, "") + "\n" + entries).strip()
    order = list(SECTIONS.values()) + sorted(set(bodies) - set(SECTIONS.values()))
    unreleased = "".join(f"\n### {heading}\n\n{bodies[heading]}\n" for heading in order if heading in bodies)
    return text[:start] + unreleased + "\n" + text[end:]


def main():
    if sys.argv[1:] not in ([], ["--merge"]):
        raise SystemExit("usage: scripts/changelog.py [--merge]")
    problems = []
    found = fragments(problems)
    if not problems and sys.argv[1:] == ["--merge"]:
        CHANGELOG.write_text(merge(found), encoding="utf-8")
        for _, path, _ in found:
            path.unlink()
    for problem in problems:
        print(problem, file=sys.stderr)
    sys.exit(1 if problems else 0)


if __name__ == "__main__":
    main()
