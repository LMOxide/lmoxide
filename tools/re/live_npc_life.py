#!/usr/bin/env python3
"""NPC life by level, as the live server gave it, from a live capture. Writes data/npc_life.json.

Usage:
    python3 tools/re/live_npc_life.py [data/live-capture/lmo-captures/playfield-objects.json]

Input: the spawned characters of tools/re/live_playfield.py (ProtoCoreCharacterIIR stats.level
and stats.life of every NPC, instance < 16,000,000; invisible_monsterdata 51057 left out).

The live server's NPC life is a life per level times a factor per monster: at each level most
NPCs share one value, and the others are fixed multiples of it (0.25x Vespid, 0.5x Mantis Nymph,
1.5x Rascal Compy, 2.25x Forest Treeman, 7.5-11.25x bosses), the same factor at every level a
monster was seen at.  BASE lists the levels' 1x values.  They are picked by hand, because the
most common value isn't always the 1x one (level 4: 123 Spider eggs at 105 = 0.8x, 113 Musketeers,
Hunters and Scouts at 132).  Each one must occur in the capture at its level (checked), and
together they must make a rising curve.  Level 30 is left out: its two values (Swashbuckler
6813, Queen's Guard 750) fit neither neighbour.  Between and beyond the listed levels the life
is interpolated linearly (npc_life.json "interpolation").

Output:
  base:     {level: life of a regular (1x) NPC}
  monsters: {monster_data: {name, factor, samples, levels}}; factor = life / base life of its
            level, rounded to 0.05, the median over the samples.
"""
import json
import os
import statistics
import sys
from collections import defaultdict

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../..")
DEFAULT_IN = os.path.join(ROOT, "data/live-capture/lmo-captures/playfield-objects.json")
OUT = os.path.join(ROOT, "data/archive/npc_life.json")
BASE = {1: 62, 2: 90, 3: 114, 4: 132, 5: 168, 6: 234, 7: 282, 10: 538, 12: 750, 14: 1002, 15: 1314,
        19: 2362, 20: 2618, 23: 3024, 26: 3854, 27: 4020, 28: 4190, 32: 5878, 43: 8412, 46: 9230,
        48: 9818, 55: 12230, 56: 12632, 60: 14416, 62: 15438, 66: 17800, 68: 19172, 71: 21526,
        74: 24298}
INVISIBLE = 51057


def base_life(level, base=BASE):
    """Life of a regular NPC of `level`: BASE, linear between its levels and beyond its ends."""
    pts = sorted(base.items())
    if level in base:
        return float(base[level])
    lo = [p for p in pts if p[0] < level]
    hi = [p for p in pts if p[0] > level]
    a, b = (lo[-1], hi[0]) if lo and hi else (pts[:2] if not lo else pts[-2:])
    return a[1] + (b[1] - a[1]) * (level - a[0]) / (b[0] - a[0])


def main():
    src = sys.argv[1] if len(sys.argv) > 1 else DEFAULT_IN
    objs = json.load(open(src))["objects"]
    seen = defaultdict(set)  # level -> lives
    samples = defaultdict(list)  # monster_data -> (level, life, name)
    for o in objs:
        c = o.get("core")
        if o["kind"] != "character" or o["instance"] >= 16_000_000 or not c or "npc_stats" not in c:
            continue
        s = c["stats"]
        if s.get("monster_data") == INVISIBLE or not s.get("life"):
            continue
        seen[s["level"]].add(s["life"])
        samples[s["monster_data"]].append((s["level"], s["life"], c["name"].strip()))
    for level, life in BASE.items():
        assert life in seen[level], f"level {level}: {life} not in the capture ({sorted(seen[level])})"
    lives = [BASE[k] for k in sorted(BASE)]
    assert lives == sorted(lives), "BASE must rise with the level"
    monsters = {}
    for md, ss in sorted(samples.items()):
        factors = [round(life / base_life(lv) * 20) / 20 for lv, life, _ in ss]
        monsters[str(md)] = dict(name=statistics.mode(n for _, _, n in ss), factor=statistics.median(factors),
                                 samples=len(ss), levels=sorted({lv for lv, _, _ in ss}))
    out = {
        "_comment": "NPC life on the live server (tools/re/live_npc_life.py, from LMO_Captures.cap): "
                    "life = base[level] x the monster's factor. Proposed enemies use it "
                    "(extract_world_content.py), the server ranks enemies by it (npc.rs).",
        "interpolation": "linear between the listed levels, and beyond the first two / last two",
        "base": {str(k): v for k, v in sorted(BASE.items())},
        "monsters": monsters,
    }
    with open(OUT, "w") as f:
        json.dump(out, f, indent=1)
        f.write("\n")
    print(f"wrote {OUT}: {len(BASE)} levels, {len(monsters)} monsters")


if __name__ == "__main__":
    main()
