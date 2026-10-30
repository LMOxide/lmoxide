"""How long each effect package holds its caster in an animation (the longest
emotionfxattribute timeline: `ability`, `channel`, ... from data/effect_catalog.json) ->
data/effect_animation_lengths.json {package: seconds}, packages of at least 0.5 s. The enemy AI
keeps a caster busy until its cast animation ends (npc_spells.rs `busy`); recasting sooner
restarted it, which looked like a freeze (the Minotaur's Smash, 3.17 s, every 2 s).
  python3 tools/re/extract_animation_lengths.py"""
import json, os
ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../..")
cat = json.load(open(os.path.join(ROOT, "data/effect_catalog.json")))["effects"]
out = {}
for e in (cat.values() if isinstance(cat, dict) else cat):
    t = [x["duration"] or 0.0 for x in e.get("timelines", []) if x["type"] == "emotionfxattribute" and not x.get("until_stopped")]
    if t and max(t) >= 0.5:
        out[str(e["id"])] = round(max(t), 2)
with open(os.path.join(ROOT, "data/effect_animation_lengths.json"), "w") as f:
    json.dump(dict(sorted(out.items(), key=lambda kv: int(kv[0]))), f, separators=(",", ":"))
    f.write("\n")
print(len(out), "packages")
