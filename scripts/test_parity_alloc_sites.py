"""Tests of scripts/parity-alloc-sites.py, on DHAT files written here."""

import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent / "parity-alloc-sites.py"
spec = importlib.util.spec_from_file_location("parity_alloc_sites", SCRIPT)
sites = importlib.util.module_from_spec(spec)
spec.loader.exec_module(sites)


def dhat(points):
    """A DHAT file's JSON: `points` as (frames, blocks)."""
    table = []
    pps = []
    for frames, blocks in points:
        indices = []
        for frame in frames:
            if frame not in table:
                table.append(frame)
            indices.append(table.index(frame))
        pps.append({"fs": indices, "tbk": blocks})
    return json.dumps({"ftbl": table, "pps": pps})


MALLOC = "0x1: malloc (in /nix/store/x-valgrind/libexec/valgrind/vgpreload_dhat-amd64-linux.so)"


def yuna(line):
    return [MALLOC, f"0x2: <yuna::state::State>::draw (frames.rs:{line})"]


MESA = [MALLOC, "0x3: _mesa_new_framebuffer (in /nix/store/y-mesa/lib/libgallium-26.3.0.so)"]


class Sites(unittest.TestCase):
    def work(self, runs):
        folder = Path(tempfile.mkdtemp())
        for name, points in runs.items():
            run = folder / "runs" / name
            run.mkdir(parents=True)
            (run / "dhat.2.json").write_text(dhat(points))
        return folder

    def test_a_site_only_the_head_allocates_at_is_named_outside(self):
        work = self.work(
            {
                "base_idle_dhat-120_1": [(yuna(10), 120)],
                "base_idle_dhat-240_1": [(yuna(10), 240)],
                "head_idle_dhat-120_1": [(yuna(12), 120), (MESA, 0)],
                "head_idle_dhat-240_1": [(yuna(12), 240), (MESA, 480)],
            }
        )
        lines = sites.report(work)
        self.assertEqual(lines[0], "idle: base 1.00, head 5.00 a frame")
        self.assertEqual(len(lines), 2, lines)
        self.assertIn("+4.00 outside: _mesa_new_framebuffer (in libgallium-26.3.0.so)", lines[1])

    def test_the_same_code_on_other_lines_is_one_site(self):
        work = self.work(
            {
                "base_partial_dhat-120_1": [(yuna(10), 120)],
                "base_partial_dhat-240_1": [(yuna(10), 240)],
                "head_partial_dhat-120_1": [(yuna(99), 120)],
                "head_partial_dhat-240_1": [(yuna(99), 240)],
            }
        )
        self.assertEqual(sites.report(work), ["partial: base 1.00, head 1.00 a frame"])

    def test_a_scenario_without_both_lengths_is_left_out(self):
        work = self.work({"base_idle_dhat-120_1": [(yuna(1), 1)]})
        self.assertEqual(sites.report(work), [])


if __name__ == "__main__":
    unittest.main()
