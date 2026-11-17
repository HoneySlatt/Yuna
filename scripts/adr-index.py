#!/usr/bin/env python3
"""Writes, or checks, the index of docs/adr/README.md from the ADRs themselves.

Each ADR's title is its heading; its status in the index is read from the `Status` lines. A status
names another ADR by a link, `[0071](0071-….md)`, or plainly, `0071` or `ADR-0071`; a date's year
is never read as one. A relation between two ADRs, one superseding the other, whole or in part, or
amending it in a project that wrote amendments before, counts when either one's status names it;
an ADR superseded in part stays in force for the rest. The check refuses a status that names an
ADR that does not exist, and a supersession, whole or in part, that both sides do not name
(docs/adr/README.md). An ADR it cannot read, or whose heading or status is malformed, is an error
that names its file. The index is written through a temporary file and a rename, and only when
there is no problem: otherwise the README is left as it was.

    scripts/adr-index.py           check that the index is the one the ADRs give
    scripts/adr-index.py --write   write it
"""

import os
import re
import sys
import tempfile
from pathlib import Path

ADR_DIRECTORY = Path(__file__).resolve().parent.parent / "docs" / "adr"
INDEX_HEADER = "| ADR | Title | Status |\n|---|---|---|\n"
# The first ADR whose supersessions both sides must name: a project that wrote them on one side
# before raises it to the first ADR written under this rule.
BOTH_SIDES_FROM = 71

LINK = re.compile(r"\[(?:ADR-)?(\d{4})\]\([^)]*\)")
# An ADR's number written plainly, `0077` or `ADR-0077`: four digits alone, so that neither a
# date's year, `2026-11-16`, nor a commit, `62568b3`, is read as one, nor a link's token again.
PLAIN = re.compile(r"(?<![\w⟦-])(?:ADR-)?(\d{4})(?![\w⟧-])")
# A number of an ADR, once made a token.
TOKEN = re.compile(r"⟦(\d{4})⟧")
# The words that start a list of ADRs in a status, longest first: "supersedes in part" before
# "supersedes", "amended by" before "amended".
RELATION_WORDS = [
    ("superseded in part by", "superseded_in_part_by"),
    ("supersedes in part", "supersedes_in_part"),
    ("superseded by", "superseded_by"),
    ("supersedes", "supersedes"),
    ("amended by", "amended_by"),
    ("it amended", "amends"),
    ("amended", "amended_by"),
    ("amends", "amends"),
]
RELATIONS = [
    "superseded_by",
    "supersedes",
    "superseded_in_part_by",
    "supersedes_in_part",
    "amends",
    "amended_by",
]
OPPOSITE = {
    "amends": "amended_by",
    "amended_by": "amends",
    "supersedes": "superseded_by",
    "superseded_by": "supersedes",
    "supersedes_in_part": "superseded_in_part_by",
    "superseded_in_part_by": "supersedes_in_part",
}
# Each supersession that both sides must name: the older side's relation, the newer side's, and
# the words a problem uses.
SUPERSESSIONS = [
    ("superseded_by", "supersedes", "supersedes"),
    ("superseded_in_part_by", "supersedes_in_part", "supersedes in part"),
]


class Malformed(Exception):
    """An ADR or a README the script cannot read as one: the message names its file."""


def outside_parentheses(text):
    """The text, each number of an ADR made a token, every parenthesis dropped with what it holds."""
    text = PLAIN.sub(r"⟦\1⟧", LINK.sub(r"⟦\1⟧", text))
    kept, depth = [], 0
    for character in text:
        if character == "(":
            depth += 1
        elif character == ")":
            depth = max(depth - 1, 0)
        elif depth == 0:
            kept.append(character)
    return " ".join("".join(kept).split())


class Adr:
    """One ADR's number, file, title, state, and the relations that its own status names."""

    def __init__(self, path):
        self.number = path.name[:4]
        self.file = path.name
        try:
            text = path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError) as error:
            raise Malformed(f"{path.name}: cannot be read: {error}") from None
        heading = re.search(r"^# (\d{4})\. (.+)$", text, re.MULTILINE)
        if not heading or heading.group(1) != self.number:
            raise Malformed(f"{path.name}: its first heading is not `# {self.number}. Title`")
        self.title = heading.group(2).strip()
        status = re.search(r"^- Status: (.*?)^- Date:", text, re.MULTILINE | re.DOTALL)
        if not status:
            raise Malformed(f"{path.name}: no `- Status:` line followed by `- Date:`")
        self.read_status(outside_parentheses(" ".join(status.group(1).split())))

    def read_status(self, text):
        state = re.match(r"[A-Za-z]+", text)
        if not state:
            raise Malformed(f"{self.file}: its status does not start with its state, such as `Accepted`: {text!r}")
        self.state = state.group(0)
        self.named = {relation: [] for relation in RELATIONS}
        # What follows the first ADR that supersedes it, up to a comma: "for the engine".
        self.supersession_note = ""
        lowered, relation, position = text.lower(), None, 0
        while position < len(text):
            word = next((pair for pair in RELATION_WORDS if lowered.startswith(pair[0], position)), None)
            token = TOKEN.match(text, position)
            if word:
                relation = word[1]
                position += len(word[0])
            elif token and relation:
                if relation == "superseded_by" and not self.named[relation]:
                    note = re.match(r"\s*([^;,]*)", text[token.end():]).group(1)
                    self.supersession_note = TOKEN.sub(r"\1", note).strip()
                self.named[relation].append(token.group(1))
                position = token.end()
            else:
                if text[position] == ";":
                    relation = None
                position += 1


def listed(numbers):
    numbers = sorted(numbers)
    return numbers[0] if len(numbers) == 1 else ", ".join(numbers[:-1]) + " and " + numbers[-1]


def index_status(adr, relations):
    superseded_by = sorted(relations["superseded_by"])
    if superseded_by:
        first = " ".join(filter(None, [superseded_by[0], adr.supersession_note]))
        rest = f", and {listed(superseded_by[1:])}" if superseded_by[1:] else ""
        parts = [f"Superseded by {first}{rest}"]
    else:
        parts = [adr.state]
    for relation, words in (
        ("supersedes", "supersedes"),
        ("supersedes_in_part", "supersedes in part"),
        ("superseded_in_part_by", "superseded in part by"),
        ("amends", "amends"),
        ("amended_by", "amended by"),
    ):
        if relations[relation]:
            parts.append(f"{words} {listed(relations[relation])}")
    return "; ".join(parts)


def relations_of(adrs, unreadable, problems):
    """Each ADR's relations, gathered from both sides; a pair superseded is not also amended. An ADR
    of `unreadable`, already a problem, is not said to be missing."""
    known = {adr.number: adr for adr in adrs}
    relations = {number: {relation: set() for relation in RELATIONS} for number in known}
    for adr in adrs:
        for relation, numbers in adr.named.items():
            for other in numbers:
                if other in unreadable:
                    continue
                if other not in known:
                    problems.append(f"{adr.file}: its status names {other}, which does not exist")
                    continue
                relations[adr.number][relation].add(other)
                relations[other][OPPOSITE[relation]].add(adr.number)
    for number, own in relations.items():
        own["amends"] -= own["supersedes"]
        own["amended_by"] -= own["superseded_by"]
        for older_side, newer_side, words in SUPERSESSIONS:
            for newer in own[older_side]:
                if int(newer) >= BOTH_SIDES_FROM:
                    if newer not in known[number].named[older_side]:
                        problems.append(f"{known[number].file}: its status does not say that {newer} {words} it")
                    if number not in known[newer].named[newer_side]:
                        problems.append(f"{known[newer].file}: its status does not say that it {words} {number}")
    return relations


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


def indexed(text, rows):
    """`text`, the README, with its index's rows replaced by `rows`."""
    header = text.find(INDEX_HEADER)
    if header == -1:
        raise Malformed("README.md: no index, a table headed `| ADR | Title | Status |`, to write the ADRs into")
    start = end = header + len(INDEX_HEADER)
    while text.startswith("| [", end):
        line_end = text.find("\n", end)
        end = len(text) if line_end == -1 else line_end + 1
    return text[:start] + rows + text[end:]


def run(directory, write):
    """Checks the index of `directory`'s README against its ADRs and, with `write`, writes it; gives
    the problems found, each naming its file, empty when all is well."""
    problems, adrs, unreadable = [], [], set()
    # The template that new ADRs copy is not one.
    for path in sorted(directory.glob("[0-9][0-9][0-9][0-9]-*.md")):
        if path.name == "0000-template.md":
            continue
        try:
            adrs.append(Adr(path))
        except Malformed as error:
            problems.append(str(error))
            unreadable.add(path.name[:4])
    relations = relations_of(adrs, unreadable, problems)
    rows = "".join(
        f"| [{adr.number}]({adr.file}) | {adr.title} | {index_status(adr, relations[adr.number])} |\n"
        for adr in adrs
    )
    readme = directory / "README.md"
    try:
        text = readme.read_text(encoding="utf-8")
        written = indexed(text, rows)
    except (OSError, UnicodeDecodeError) as error:
        return problems + [f"README.md: cannot be read: {error}"]
    except Malformed as error:
        return problems + [str(error)]
    if written == text:
        return problems
    if not write:
        return problems + ["README.md's index is not the one the ADRs give: run scripts/adr-index.py --write"]
    if problems:
        return problems + ["README.md was left as it was: its index is written once the problems above are fixed"]
    try:
        write_atomically(readme, written)
    except OSError as error:
        return [f"README.md could not be written, and was left as it was: {error}"]
    return []


def main():
    if sys.argv[1:] not in ([], ["--write"]):
        raise SystemExit("usage: scripts/adr-index.py [--write]")
    problems = run(ADR_DIRECTORY, write=sys.argv[1:] == ["--write"])
    for problem in problems:
        print(problem, file=sys.stderr)
    sys.exit(1 if problems else 0)


if __name__ == "__main__":
    main()
