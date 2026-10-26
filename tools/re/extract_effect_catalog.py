"""Catalog of the client's effect packages and particle systems -> data/effect_catalog.json.

  tools/re/py extract_effect_catalog.py [out.json] [--no-refs]

RDB 1000622 holds two kinds of XML resources: <effectpackage> (what EffectPackageIIR starts on a
character or a spot: timelines of particles, sounds, animation attributes, Lua, ...) and
<particlesystem> (a particle effect a package refers to by its resource name). Each entry lists
what it plays; `used_by` lists the item templates (RDB 1000620) and spells/nanos (1000621) whose
data mention the id (a byte search, so a few may be coincidences; --no-refs skips the slow scan).
Names come from the items/spells that use a package; packages have no names of their own.

The test command `!fx <id>` plays a package on the player (devcmd.rs); this file is also the data
for a catalog browser (docs/EFFECT-BROWSER.md).
"""
import json, os, re, struct, sys

import rdb
from item_template import parse as parse_item

HERE = os.path.dirname(os.path.abspath(__file__))
EFFECTS, ITEMS, SPELLS = 1000622, 1000620, 1000621


def text(b):
    return b.decode("utf-8", "replace")


def package(x):
    timelines = []
    for m in re.finditer(r'<effect type="([^"]+)" name="([^"]*)"[^>]*?start="([^"]*)" duration="([^"]*)"[^>]*?target="([^"]*)"', x):
        timelines.append({"type": m.group(1), "name": m.group(2), "start": float(m.group(3) or 0),
                          "duration": float(m.group(4) or 0), "target": m.group(5)})
    lua = [l.strip() for l in re.findall(r"<lua>(.*?)</lua>", x, re.S) if l.strip()]
    return {
        "kind": "effectpackage",
        "timelines": timelines,
        "particles": sorted(set(n for n in re.findall(r"<resourcename>([^<]*)</resourcename>", x) if not n.lower().endswith(".wav"))),
        "sounds": sorted(set(re.findall(r"[\w\\./ -]+\.wav", x))),
        "animation_attributes": sorted(set(re.findall(r"<attributename>([^<]*)</attributename>", x))),
        "code_hooks": sorted(set(re.findall(r'type="code_hook"[^>]*>\s*<value><!\[CDATA\[([^\]]*)', x))),
        "lua": [l[:400] for l in lua],
        "looping": "<looping><![CDATA[True]]>" in x,
    }


def particles(x):
    return {
        "kind": "particlesystem",
        "emitters": sorted(set(re.findall(r'<emitter name="([^"]*)"', x))),
        "scripts": sorted(set(s.strip() for s in re.findall(r"<scriptname>([^<]*)</scriptname>", x))),
    }


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    out_path = args[0] if args else os.path.join(HERE, "../../data/effect_catalog.json")
    idx = rdb.index()
    out = {}
    for t, r in sorted(idx):
        if t != EFFECTS:
            continue
        try:
            x = text(rdb.body(t, r, idx))
        except Exception:
            continue
        if "<effectpackage" in x:
            out[r] = {"id": r, **package(x)}
        elif "<particlesystem" in x:
            out[r] = {"id": r, **particles(x)}
    if "--no-refs" not in sys.argv:
        needles = {r: (struct.pack("<I", r), struct.pack(">I", r)) for r in out}
        for t in (ITEMS, SPELLS):
            for tt, r in idx:
                if tt != t:
                    continue
                try:
                    b = rdb.body(tt, r, idx)
                except Exception:
                    continue
                name = parse_item(b)[0] if t == ITEMS else None
                if name is None:
                    m = re.search(rb"([\x03-\x60])\x00([A-Za-z][ -~]{2,95})", b)
                    name = m.group(2).decode() if m else None
                for e, (le, be) in needles.items():
                    if le in b or be in b:
                        out[e].setdefault("used_by", []).append({"rdb": "item" if t == ITEMS else "spell", "id": r, "name": name})
    catalog = {"_comment": __doc__.strip().splitlines()[0] + " See tools/re/extract_effect_catalog.py.",
               "effects": [out[k] for k in sorted(out)]}
    json.dump(catalog, open(out_path, "w"), indent=1)
    # the package ids alone, for the server's `!fx next`
    ids = [e["id"] for e in catalog["effects"] if e["kind"] == "effectpackage"]
    json.dump(ids, open(os.path.join(os.path.dirname(out_path), "effect_package_ids.json"), "w"))
    print(out_path, len(out), sum(1 for e in out.values() if e["kind"] == "effectpackage"), "packages")


if __name__ == "__main__":
    main()
