#!/usr/bin/env python3
"""Writes, or checks, the index of docs/adr/README.md from the ADRs themselves.

Each ADR's title is its heading; its status in the index is read from the `Status` lines. A
relation between two ADRs, one superseding the other, or amending it in a project that wrote
amendments before, counts when either one's status names it. The check refuses a status that names
an ADR that does not exist, and a supersession that both sides do not name (docs/adr/README.md).

    scripts/adr-index.py           check that the index is the one the ADRs give
    scripts/adr-index.py --write   write it
"""

import re
import sys
from pathlib import Path

ADR_DIRECTORY = Path(__file__).resolve().parent.parent / "docs" / "adr"
README = ADR_DIRECTORY / "README.md"
INDEX_HEADER = "| ADR | Title | Status |\n|---|---|---|\n"
# The first ADR whose supersessions both sides must name: a project that wrote them on one side
# before raises it to the first ADR written under this rule.
BOTH_SIDES_FROM = 71

LINK = re.compile(r"\[(?:ADR-)?(\d{4})\]\([^)]*\)")
# A link to an ADR, once made a token: so that a date's year is not read as one.
TOKEN = re.compile(r"⟦(\d{4})⟧")
# The words that start a list of ADRs in a status, longest first: "amended by" before "amended".
RELATION_WORDS = [
    ("superseded by", "superseded_by"),
    ("supersedes", "supersedes"),
    ("amended by", "amended_by"),
    ("it amended", "amends"),
    ("amended", "amended_by"),
    ("amends", "amends"),
]
RELATIONS = ["superseded_by", "supersedes", "amends", "amended_by"]
OPPOSITE = {
    "amends": "amended_by",
    "amended_by": "amends",
    "supersedes": "superseded_by",
    "superseded_by": "supersedes",
}


def outside_parentheses(text):
    """The text, each link to an ADR made a token, every parenthesis dropped with what it holds."""
    text = LINK.sub(r"⟦\1⟧", text)
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
        text = path.read_text(encoding="utf-8")
        heading = re.search(r"^# (\d{4})\. (.+)$", text, re.MULTILINE)
        if not heading or heading.group(1) != self.number:
            raise SystemExit(f"{path.name}: its first heading is not `# {self.number}. Title`")
        self.title = heading.group(2).strip()
        status = re.search(r"^- Status: (.*?)^- Date:", text, re.MULTILINE | re.DOTALL)
        if not status:
            raise SystemExit(f"{path.name}: no `- Status:` line followed by `- Date:`")
        self.read_status(outside_parentheses(" ".join(status.group(1).split())))

    def read_status(self, text):
        self.state = re.match(r"[A-Za-z]+", text).group(0)
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
    for relation, words in (("supersedes", "supersedes"), ("amends", "amends"), ("amended_by", "amended by")):
        if relations[relation]:
            parts.append(f"{words} {listed(relations[relation])}")
    return "; ".join(parts)


def relations_of(adrs, problems):
    """Each ADR's relations, gathered from both sides; a pair superseded is not also amended."""
    known = {adr.number: adr for adr in adrs}
    relations = {number: {relation: set() for relation in RELATIONS} for number in known}
    for adr in adrs:
        for relation, numbers in adr.named.items():
            for other in numbers:
                if other not in known:
                    problems.append(f"{adr.number}'s status names {other}, which does not exist")
                    continue
                relations[adr.number][relation].add(other)
                relations[other][OPPOSITE[relation]].add(adr.number)
    for number, own in relations.items():
        own["amends"] -= own["supersedes"]
        own["amended_by"] -= own["superseded_by"]
        for newer in own["superseded_by"]:
            if int(newer) >= BOTH_SIDES_FROM:
                if newer not in known[number].named["superseded_by"]:
                    problems.append(f"{number}'s status does not say that {newer} supersedes it")
                if number not in known[newer].named["supersedes"]:
                    problems.append(f"{newer}'s status does not say that it supersedes {number}")
    return relations


def main():
    if sys.argv[1:] not in ([], ["--write"]):
        raise SystemExit("usage: scripts/adr-index.py [--write]")
    paths = sorted(ADR_DIRECTORY.glob("[0-9][0-9][0-9][0-9]-*.md"))
    # The template that new ADRs copy is not one.
    adrs = [Adr(path) for path in paths if path.name != "0000-template.md"]
    problems = []
    relations = relations_of(adrs, problems)
    rows = "".join(
        f"| [{adr.number}]({adr.file}) | {adr.title} | {index_status(adr, relations[adr.number])} |\n"
        for adr in adrs
    )
    text = README.read_text(encoding="utf-8")
    start = text.index(INDEX_HEADER) + len(INDEX_HEADER)
    end = start
    while text.startswith("| [", end):
        end = text.index("\n", end) + 1
    written = text[:start] + rows + text[end:]
    if sys.argv[1:] == ["--write"]:
        README.write_text(written, encoding="utf-8")
    elif written != text:
        problems.append("docs/adr/README.md's index is not the one the ADRs give: run scripts/adr-index.py --write")
    for problem in problems:
        print(problem, file=sys.stderr)
    sys.exit(1 if problems else 0)


if __name__ == "__main__":
    main()
