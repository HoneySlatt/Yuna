"""Per-frame allocations by call site, head minus base, from a parity work folder's DHAT runs."""
import glob, json, os, re, sys
from collections import defaultdict

def blocks(path, depth=8):
    d = json.load(open(path))
    ftbl = d["ftbl"]
    out = defaultdict(int)
    for pp in d["pps"]:
        frames = [ftbl[i] for i in pp["fs"]]
        frames = [f for f in frames if "vgpreload" not in f and "[root]" not in f and "alloc.rs" not in f and "raw_vec" not in f][:depth]
        names = [re.sub(r"\(in [^)]*/([^/)]+)\)", r"(in \1)", f.split(": ", 1)[-1])[:110] for f in frames]
        out[" | ".join(names)] += pp["tbk"]
    return out

def per_frame(work, side, scenario):
    files = {}
    for n in (120, 240):
        found = glob.glob(os.path.join(work, "runs", f"{side}_{scenario}_dhat-{n}_1", "**", "*dhat*"), recursive=True)
        found = [f for f in found if os.path.isfile(f)]
        if not found:
            sys.exit(f"no dhat file for {side} {scenario} {n}: {os.listdir(os.path.join(work, 'runs'))}")
        files[n] = blocks(found[0])
    keys = set(files[120]) | set(files[240])
    return {k: (files[240].get(k, 0) - files[120].get(k, 0)) / 120 for k in keys}

work, scenario = sys.argv[1], sys.argv[2]
base, head = per_frame(work, "base", scenario), per_frame(work, "head", scenario)
print(f"total a frame: base {sum(base.values()):.2f}, head {sum(head.values()):.2f}")
diffs = sorted(((head.get(k, 0) - base.get(k, 0), k) for k in set(base) | set(head)), key=lambda x: -abs(x[0]))
for d, k in diffs[:25]:
    if abs(d) >= 0.05:
        print(f"{d:+.2f} a frame (base {base.get(k,0):.2f}, head {head.get(k,0):.2f}): {k}")
