#!/usr/bin/env python3
"""Prints, for each scenario of a parity run, where the head allocates more a frame than the base.

The parity script (`yuna-bench parity`, ADR-0142) judges a scenario's allocations a frame as one
figure; when it flags one, this says which call sites moved. It reads the DHAT files a run kept in
its work folder (`parity run --keep`, CI's `parity` job): for each side and scenario, the runs of
two lengths, `<side>_<scenario>_dhat-<frames>_<repeat>`, and takes a site's blocks a frame as the
difference of the two lengths' blocks over the difference of their frames, averaged over repeats.

A site is its first frames past the allocator, with line numbers, generic arguments and module
paths cut, so that the same code in base and head, whose lines and crates moved, reads as one
site. Sites outside Yuna's own code (no Rust source among their frames) are marked `outside`.

    scripts/parity-alloc-sites.py <work folder>
"""

import json
import re
import sys
from collections import defaultdict
from pathlib import Path
from statistics import mean

RUN = re.compile(r"^(base|head)_(.+)_dhat-(\d+)_(\d+)$")
DEPTH = 8
SHOWN = 12
SMALLEST = 0.05


def frame_name(frame):
    """A frame's function, without its address, line, generic arguments or module path."""
    text = frame.split(": ", 1)[-1]
    text = re.sub(r"\(in [^)]*/([^/)]+)\)", r"(in \1)", text)
    text = re.sub(r":\d+\)", ")", text)
    while True:
        cut = re.sub(r"<[^<>]*>", "", text)
        if cut == text:
            break
        text = cut
    return re.sub(r"\b[a-z_][a-z0-9_]*::", "", text)


def is_allocator(frame):
    return any(part in frame for part in ("vgpreload", "alloc.rs", "raw_vec", "[root]"))


def sites(path):
    """Blocks allocated at each site of the DHAT file at `path`, and whether the site is Yuna's."""
    data = json.loads(Path(path).read_text())
    table = data["ftbl"]
    blocks = defaultdict(int)
    own = {}
    for point in data["pps"]:
        frames = [table[i] for i in point["fs"]]
        kept = [f for f in frames if not is_allocator(f)][:DEPTH]
        site = " | ".join(frame_name(f) for f in kept)
        blocks[site] += point["tbk"]
        own[site] = any(".rs:" in f for f in frames)
    return blocks, own


def per_frame(runs):
    """A site's blocks a frame, from `runs`: {frames: [DHAT file]} of at least two lengths."""
    lengths = sorted(runs)
    short, long = lengths[0], lengths[-1]
    totals = {}
    own = {}
    for length in (short, long):
        by_site = defaultdict(list)
        for path in runs[length]:
            blocks, own_sites = sites(path)
            own.update(own_sites)
            for site, count in blocks.items():
                by_site[site].append(count)
        totals[length] = {site: mean(counts) for site, counts in by_site.items()}
    every = set(totals[short]) | set(totals[long])
    span = long - short
    return {s: (totals[long].get(s, 0) - totals[short].get(s, 0)) / span for s in every}, own


def collect(work):
    """{scenario: {side: {frames: [DHAT file]}}} from a work folder's runs."""
    found = defaultdict(lambda: defaultdict(lambda: defaultdict(list)))
    for run in sorted((Path(work) / "runs").iterdir()):
        match = RUN.match(run.name)
        if not match:
            continue
        side, scenario, frames, _ = match.groups()
        for path in run.glob("dhat*.json"):
            found[scenario][side][int(frames)].append(path)
    return found


def report(work):
    lines = []
    for scenario, sides in sorted(collect(work).items()):
        if not all(len(sides.get(side, {})) >= 2 for side in ("base", "head")):
            continue
        base, own = per_frame(sides["base"])
        head, head_own = per_frame(sides["head"])
        own.update(head_own)
        lines.append(
            f"{scenario}: base {sum(base.values()):.2f}, head {sum(head.values()):.2f} a frame"
        )
        moved = [(head.get(s, 0) - base.get(s, 0), s) for s in set(base) | set(head)]
        moved = [m for m in moved if abs(m[0]) >= SMALLEST]
        for change, site in sorted(moved, key=lambda m: -abs(m[0]))[:SHOWN]:
            where = "yuna" if own.get(site) else "outside"
            lines.append(f"  {change:+.2f} {where}: {site}")
    return lines


def main(argv):
    if len(argv) != 2:
        print(__doc__.strip().splitlines()[-1].strip(), file=sys.stderr)
        return 2
    for line in report(argv[1]):
        print(line)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
