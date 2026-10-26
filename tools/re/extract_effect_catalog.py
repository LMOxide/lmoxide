"""Catalog of the client's effect packages and particle systems -> data/effect_catalog.json.

  tools/re/py extract_effect_catalog.py [out.json] [--no-refs | --keep-refs]

RDB 1000622 holds two kinds of XML resources: <effectpackage> (what EffectPackageIIR starts on a
character or a spot: timelines of particles, sounds, animation attributes, Lua, ...) and
<particlesystem> (a particle effect a package refers to by its resource name). Each entry lists
what it plays; `used_by` lists the item templates (RDB 1000620) and spells/nanos (1000621) whose
data mention the id (a byte search, so a few may be coincidences; --no-refs skips the slow scan,
--keep-refs takes `used_by` from the current output file instead).
Names come from the items/spells that use a package; packages have no names of their own.

Timeline parts carry the code hook that starts their <timeline> (`hook`: onhit, charge, ...;
null: when the package starts) and their duration in seconds, null when they set none
(use_duration="False") or last until the package is stopped (`until_stopped`: the designers'
86400). Particles and sounds carry their RDB ids (1010207, 1020002), looked up in the client's
name index (tools/re/rdb_names.py); packages name some particles only by id.

The test command `!fx <id>` plays a package on the player (devcmd.rs); this file is also the data
for a catalog browser (docs/EFFECT-BROWSER.md).
"""
import json, os, re, struct, sys

import rdb

import rdb_names
from item_template import parse as parse_item

HERE = os.path.dirname(os.path.abspath(__file__))
EFFECTS, ITEMS, SPELLS, PARTICLES, SOUNDS = 1000622, 1000620, 1000621, 1010207, 1020002


def text(b):
    return b.decode("utf-8", "replace")


# a part's duration in the designers' "forever": it lasts until the package is stopped
FOREVER = 86400


def package(x, particle_name, sound_id):
    timelines = []
    # each <timeline> may start at a code hook (<trigger type="code_hook" value="onhit">); its
    # <effect>s are the parts
    for tl in re.finditer(r"<timeline .*?</timeline>", x, re.S):
        block = tl.group(0)
        hook = re.search(r'type="code_hook" value="([^"]*)"', block)
        for m in re.finditer(r'<effect type="([^"]+)" name="([^"]*)"([^>]*)>', block):
            a = dict(re.findall(r'(\w+)="([^"]*)"', m.group(3)))
            duration = float(a.get("duration") or 0)
            until_stopped = duration >= FOREVER
            timelines.append({
                "type": m.group(1), "name": m.group(2), "start": float(a.get("start") or 0),
                # null: no duration of its own (use_duration="False": as long as its content, e.g.
                # a sound) or until the package is stopped (until_stopped)
                "duration": None if until_stopped or a.get("use_duration") == "False" else duration,
                "until_stopped": until_stopped,
                "target": a.get("target", ""),
                "hook": hook.group(1) if hook else None,
            })
    lua = [l.strip() for l in re.findall(r"<lua>(.*?)</lua>", x, re.S) if l.strip()]
    names = sorted(set(n for n in re.findall(r"<resourcename>([^<]*)</resourcename>", x) if not n.lower().endswith(".wav")))
    return {
        "kind": "effectpackage",
        "timelines": timelines,
        # by RDB 1010207 id and name; packages name some by id, some by name (the other is
        # looked up in the client's name index, tools/re/rdb_names.py; null if it isn't there)
        "particles": [particle_name(n) for n in names],
        # by path, with the RDB 1020002 id (Ogg Vorbis) the name index gives it
        "sounds": [{"path": p, "id": sound_id(p)} for p in sorted(set(re.findall(r"[\w\\./ -]+\.wav", x)))],
        "animation_attributes": sorted(set(re.findall(r"<attributename>([^<]*)</attributename>", x))),
        "code_hooks": sorted(set(re.findall(r'type="code_hook" value="([^"]*)"', x))),
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
    # (exact for particles: rebuilding the index from these names reproduces it)
    ids = rdb_names.names(PARTICLES)

    def particle_name(n):
        if n.isdigit():
            return {"id": int(n), "name": ids.get(int(n))}
        # as the client does (LMO.exe 0x46e490): the name, else with ".particleeffect"
        return {"id": rdb_names.lookup(PARTICLES, n) or rdb_names.lookup(PARTICLES, n + ".particleeffect"), "name": n}

    out = {}
    for t, r in sorted(idx):
        if t != EFFECTS:
            continue
        try:
            x = text(rdb.body(t, r, idx))
        except Exception:
            continue
        if "<effectpackage" in x:
            out[r] = {"id": r, **package(x, particle_name, lambda p: rdb_names.lookup(SOUNDS, p))}
        elif "<particlesystem" in x:
            out[r] = {"id": r, **particles(x)}
    if "--keep-refs" in sys.argv:
        old = {e["id"]: e.get("used_by") for e in json.load(open(out_path))["effects"]}
        for r, e in out.items():
            if old.get(r):
                e["used_by"] = old[r]
    elif "--no-refs" not in sys.argv:
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
