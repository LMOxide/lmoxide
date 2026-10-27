#!/usr/bin/env python3
"""Checks a content tree made by `lmoxide-content import` against the data files it came from:
every placement, portal, jump pad, spell, item, minifigure, achievement and dialogue must be
there with the same values. Usage: tools/content/verify_import.py [data] [content]"""
import glob
import json
import os
import sys

data = sys.argv[1] if len(sys.argv) > 1 else "data"
content = sys.argv[2] if len(sys.argv) > 2 else "content"
problems = []
checked = {}


def load(path):
    with open(path) as f:
        return json.load(f)


def d(name):
    archived = os.path.join(data, "archive", name)
    return load(archived if os.path.exists(archived) else os.path.join(data, name))


records = {}
world_of = {}
for f in glob.glob(os.path.join(content, "**", "*.json"), recursive=True):
    rel = os.path.relpath(f, content)
    if rel in ("content.json", "tunables.json", "texts.json") or rel.startswith("schemas"):
        continue
    v = load(f)
    recs = [v] if "id" in v else [r for sec in v.values() for r in sec]
    for r in recs:
        records[r["id"]] = r
        if rel.startswith("worlds/"):
            world_of[r["id"]] = rel.split("/")[1]


def merge(base, patch):
    if isinstance(base, dict) and isinstance(patch, dict):
        out = dict(base)
        for k, v in patch.items():
            if v is None:
                out.pop(k, None)
            else:
                out[k] = merge(out.get(k), v)
        return out
    return patch


def count(what):
    checked[what] = checked.get(what, 0) + 1


def near(a, b, eps=0.01):
    return a is not None and b is not None and all(abs(x - y) <= eps for x, y in zip(a, b))


worlds = {r["playfield"]: r for r in records.values() if r["id"].startswith("world/")}
folder = {pf: world_of[w["id"]] for pf, w in worlds.items()}
by_world = {}
for r in records.values():
    if r["id"].split("/")[0] in ("spawn", "portal", "bouncer", "area") and r["id"] in world_of:
        by_world.setdefault((world_of[r["id"]], r["id"].split("/")[0]), []).append(r)

# placements
wc = d("world_content.json")
for pf, w in wc.items():
    if not isinstance(w, dict):
        continue
    pf = int(pf)
    spawns = by_world.get((folder[pf], "spawn"), [])
    for section in ("enemies", "npcs", "triggers", "live_dynamic", "smashables", "buildies"):
        for e in w.get(section, []):
            count(f"placements.{section}")
            cands = [s for s in spawns if near(s["pos"], e["pos"]) and s.get("name") == e["name"]]
            if not cands:
                problems.append(f"{pf} {section}: no spawn for {e['name']} at {e['pos']}")
                continue
            ok = False
            for s in cands:
                t = merge(records[s["template"]], s.get("overrides") or {})
                health = t.get("stats", {}).get("health") if s["template"].startswith("npc/") else t.get("health")
                render = t.get("render", {})
                if (render.get("monster_data") == e.get("monster_data") and t.get("level") == e.get("level")
                        and health == e.get("health") and render.get("cars_group") == e.get("cars_group", render.get("cars_group"))
                        and (e.get("template") is None or render.get("template") == e.get("template"))
                        and (e.get("npc_template") is None or render.get("npc_template") == e.get("npc_template"))
                        and s.get("patrol", []) == e.get("patrol", [])):
                    ok = True
            if not ok:
                problems.append(f"{pf} {section}: {e['name']} at {e['pos']}: values differ")
    for section in ("simple_dynels", "doors"):
        for e in w.get(section, []):
            count(f"placements.{section}")
            if e.get("kind") == "jump_pad":
                pads = by_world.get((folder[pf], "bouncer"), [])
                b = [b for b in pads if b.get("client", {}).get("guid") == e["guid"]]
                if not b or b[0].get("target") != e.get("target") or b[0].get("apex") != e.get("apex"):
                    problems.append(f"{pf} jump pad {e['name']}: missing or different")
                continue
            s = [s for s in spawns if s.get("client", {}).get("guid") == e["guid"] and near(s["pos"], e["pos"])]
            if not s or s[0].get("client", {}).get("visual") != e.get("visual") or s[0].get("rot") != e.get("rot"):
                problems.append(f"{pf} {section}: {e['name']} guid {e['guid']} missing or different")
    for e in w.get("live_dynels", []) + w.get("demo_dynels", []):
        count("placements.dynels")
        s = [s for s in spawns if near(s["pos"], e["pos"]) and s.get("client", {}).get("dynel_type") == e["type"]]
        if not s:
            problems.append(f"{pf} dynel {e['name']} missing")
    for e in w.get("static_points", []):
        count("points")
        a = [a for a in by_world.get((folder[pf], "area"), []) if a.get("name") == e["name"] and near(a["shape"]["point"]["pos"], e["pos"])]
        if not a:
            problems.append(f"{pf} point {e['name']} missing")
    for e in w.get("portals", []):
        count("portals")
        p = [p for p in by_world.get((folder[pf], "portal"), []) if near(p["pos"], e["pos"], 0.5) and records[p["to"]["world"]]["playfield"] == e["to"]]
        if not p:
            problems.append(f"{pf} portal to {e['to']} missing")

# spells
sa = d("spell_actions.json")
skills = {r["client"]["spell"]: r for r in records.values() if r["id"].startswith("skill/") and "client" in r}
for k, v in list(sa.items()) + list(d("npc_spells.json")["spells"].items()):
    if not k.isdigit():
        continue
    count("skills")
    s = skills.get(int(k))
    if not s or s["client"].get("stats", {}) != v["stats"]:
        problems.append(f"skill {k} missing or its stats differ")
        continue
    lists = {n: [{"type": a["type"], **({"client": True} if a.get("client") else {}), **({"args": a["args"]} if a.get("args") else {}), **({"criteria": a["criteria"]} if a.get("criteria") else {})} for a in l] for n, l in v["lists"].items()}
    if s["client"].get("actions", {}) != lists:
        problems.append(f"skill {k}: actions differ")
for k, p in d("projectiles.json").items():
    count("projectiles")
    s = skills.get(int(k))
    if not s or s.get("projectile", {}).get("speed") != p["speed"] or s["projectile"].get("item") != p["item"]:
        problems.append(f"projectile of {k} missing or different")

# items
items = {r["template"]: r for r in records.values() if r["id"].startswith("item/") and "template" in r}
li = d("loot_items.json")
for t in li["token_items"]:
    count("items.tokens")
    i = items.get(t["template"])
    give = {"token": t["token"], **({"amount": t["amount"]} if t.get("amount") is not None else {})}
    if not i or give not in i.get("tokens", []):
        problems.append(f"token item {t['template']} missing or different")
for b in li["minifigure_bags"]:
    count("items.bags")
    i = items.get(b["bag_template"])
    named = records.get(i.get("minifigure"), {}).get("template") == b["minifigure_template"] or str(b["minifigure_template"]) in i.get("comment", "") if i else False
    if not i or i["kind"] != "minifig_bag" or (b["minifigure_template"] and not named):
        problems.append(f"bag {b['bag_template']} missing or different")
for c in li["chests"]:
    count("items.chests")
    i = items.get(c["template"])
    if not i or i["chest"].get("loot_type") != c["loot_type"]:
        problems.append(f"chest {c['template']} missing or different")

# minifigures
ms = d("minifigure_stats.json")["minifigures"]
minis = {r["template"]: r for r in records.values() if r["id"].startswith("minifig/")}
for k, m in ms.items():
    count("minifigures")
    r = minis.get(int(k))
    if not r:
        problems.append(f"minifigure {k} missing")
        continue
    for f in ("life", "power", "defense", "creativity", "role", "element"):
        if r["stats"][f] != m[f]:
            problems.append(f"minifigure {k} {f} differs")
    if skills.get(m["normal"]["spell"], {}).get("id") != r["normal"]["skill"]:
        problems.append(f"minifigure {k} normal differs")
    ups = [(u["bit"], records[u["buff"]].get("nano")) for u in r.get("upgrades", [])]
    if ups != [(u["bit"], u["ability"]) for u in m["upgrades"]]:
        problems.append(f"minifigure {k} upgrades differ")
    parts = [records[p["item"]]["template"] for p in r.get("parts", [])]
    exp = [p["part_template_id"] for mm in d("minifig_parts.json")["minifigures"] if mm["minifigure_template_id"] == int(k) for p in mm["parts"]]
    if parts != exp:
        problems.append(f"minifigure {k} parts differ")

# achievements, dialogues, worlds
achs = {r["lore"]: r for r in records.values() if r["id"].startswith("achievement/") and "lore" in r}
for a in d("achievements.json")["achievements"]:
    count("achievements")
    r = achs.get(a["id"])
    if not r or r.get("title") != a.get("title") or r.get("points") != a.get("points") or r.get("counter") != a.get("counter"):
        problems.append(f"achievement {a['id']} missing or different")
for x in d("dialogues.json")["dialogues"]:
    count("dialogues")
    if not [r for r in records.values() if r["id"].startswith("dialogue/") and r["texts"] == x["texts"]]:
        problems.append(f"dialogue {x['texts'][:2]} missing")
for pf, dest in d("destinations.json")["destinations"].items():
    count("worlds")
    w = worlds.get(int(pf))
    if not w or (dest["window"] and w.get("window") != dest["window"]) or (w.get("arrival") and w["arrival"]["pos"] != dest["pos"]):
        problems.append(f"world {pf} missing or different")
for pf, pts in d("resurrection_points.json").items():
    if pf.isdigit() and int(pf) in worlds:
        for p in pts:
            count("respawn_points")
            if not [q for q in worlds[int(pf)].get("respawn_points", []) if q.get("guid") == p["guid"] and q["pos"] == p["pos"]]:
                problems.append(f"respawn point {p['guid']} of {pf} missing")

# encounters, puzzles, fights
for pf, e in d("encounters.json").items():
    if not isinstance(e, dict):
        continue
    areas = by_world.get((folder[int(pf)], "area"), [])
    for section, role in (("arenas", "arena"), ("ambushes", "ambush"), ("explore", "explore")):
        for a in e.get(section, []):
            count("encounter_areas")
            pts = [list(p) for p in a["polygon"]]
            if not [x for x in areas if x.get("role") == role and x.get("name") == a["name"] and x["shape"]["polygon"]["points"] == pts]:
                problems.append(f"{pf} {section} {a['name']} missing")
    if e.get("champion"):
        count("encounter_areas")
        if not [x for x in areas if x.get("role") == "champion" and x["shape"]["ground"]["at"] == e["champion"]]:
            problems.append(f"{pf} champion point missing")
puzzles = [r for r in records.values() if r["id"].startswith("puzzle/")]
mirrors = d("mirror_puzzles.json")
for pf, m in mirrors.items():
    if pf.isdigit():
        count("puzzles")
        if not [p for p in puzzles if world_of[p["id"]] == folder[int(pf)] and "mirrors" in p["puzzle"] and len(p["puzzle"]["mirrors"]["mirrors"]) == len(m["mirrors"])]:
            problems.append(f"mirror puzzle {pf} missing")
for pf, gs in d("light_gates.json").items():
    if pf.isdigit():
        for g in gs:
            count("puzzles")
            if not [p for p in puzzles if p["puzzle"].get("light_gate", {}).get("gate") == g["gate"]]:
                problems.append(f"light gate {g['gate']} missing")
for pf, f in d("npc_fights.json").items():
    if not pf.isdigit():
        continue
    spawns = by_world.get((folder[int(pf)], "spawn"), [])
    if worlds[int(pf)].get("npc_fights", {}).get("rivals") != f.get("rivals", None) and f.get("rivals"):
        problems.append(f"npc fights of {pf}: rivals differ")
    for section, role in (("enemies", "enemy"), ("fighters", "fighter")):
        for e in f.get(section, []):
            count("npc_fights")
            if not [s for s in spawns if s.get("fight") == role and near(s["pos"], e["pos"])]:
                problems.append(f"npc fight {pf} {e['name']} missing")

# enemy skills, life factors, summons, dashes
npcs = [r for r in records.values() if r["id"].startswith("npc/")]
ns = d("npc_spells.json")
life = d("npc_life.json")["monsters"]
for md, m in ns["monsters"].items():
    for n in [n for n in npcs if n.get("render", {}).get("monster_data") == int(md) and n.get("kind") != "summon"]:
        count("npc_skills")
        got = [records[s["skill"]]["client"]["spell"] for s in n.get("skills", [])]
        if got != m["spells"]:
            problems.append(f"{n['id']}: skills {got} != {m['spells']}")
for md, m in life.items():
    for n in [n for n in npcs if n.get("render", {}).get("monster_data") == int(md) and n.get("kind") != "summon"]:
        count("life_factors")
        if abs(n.get("stats", {}).get("life_factor", -1) - m["factor"]) > 1e-6:
            problems.append(f"{n['id']}: life factor differs")
for k, sm in d("summons.json").items():
    if k.isdigit():
        count("summons")
        s = skills.get(int(k))
        if not s or records[s["summon"]["npc"]]["render"]["monster_data"] != sm["monster_data"]:
            problems.append(f"summon {k} missing or different")
for k, ds in d("dashes.json").items():
    count("dashes")
    s = skills.get(int(k))
    if not s or s.get("dash", {}).get("distance") != ds["distance"]:
        problems.append(f"dash {k} missing or different")

# minifigure extras
auras = d("minifigure_auras.json")
drops = {m["template"]: m for m in d("minifigure_drops.json")["minifigures"]}
for t, r in minis.items():
    count("minifigure_extras")
    if auras.get(str(t), []) != r.get("aura_tags", []):
        problems.append(f"minifigure {t} auras differ")
    dr = drops.get(t)
    if dr and (r.get("obtain", {}).get("source") != dr["source"] or len(r["obtain"].get("regions", [])) != len(dr.get("worlds", []))):
        problems.append(f"minifigure {t} drop source differs")

# return points
for pf, froms in d("return_points.json").items():
    if not pf.isdigit():
        continue
    for frm, p in froms.items():
        count("return_points")
        wid = worlds.get(int(frm), {}).get("id")
        if worlds[int(pf)].get("arrival_from", {}).get(wid, {}).get("pos") != p["pos"]:
            problems.append(f"return point {pf} from {frm} differs")

# the world editor's set
survey = os.path.join(os.path.dirname(os.path.abspath(data)), "..", "var", "survey-set", "published")
for f in glob.glob(os.path.join(survey, "*.placements.json")):
    v = load(f)
    for p in v.get("placements", []):
        count("survey_placements")
        kind = "bouncer" if p["kind"] == "bouncer" else "spawn"
        r = records.get(f"{kind}/{v['playfield']}/{p['id']}")
        if not r or r["pos"] != p["pos"]:
            problems.append(f"survey placement {p['id']} missing")
    for w in v.get("triggers", []):
        count("survey_triggers")
        if f"trigger/{v['playfield']}/{w['id']}" not in records:
            problems.append(f"survey trigger {w['id']} missing")
for f in glob.glob(os.path.join(survey, "*.notes.json")):
    v = load(f)
    for sc in v.get("scripts", []):
        count("survey_scripts")
        if not [r for r in records.values() if r["id"].startswith("script/") and [w["id"] for w in r.get("waves", [])] == [w["id"] for w in sc["waves"]]]:
            problems.append(f"survey script {sc['id']} missing")

# texts, tunables
texts = load(os.path.join(content, "texts.json"))
dl = d("dialogues.json")
for section in ("texts", "answers", "keys", "names"):
    count("texts")
    if texts[section] != dl[section]:
        problems.append(f"texts.json {section} differ")
tun = load(os.path.join(content, "tunables.json"))
count("tunables")
if tun["npc_life"]["base"] != d("npc_life.json")["base"] or tun["minifigure_model"] != d("minifigure_stats.json")["model"]:
    problems.append("tunables differ")
if tun["story_quests"] != {k: v for k, v in d("story_quests.json").items() if not k.startswith("_")}:
    problems.append("story quests differ")
for a in d("achievements.json")["achievements"]:
    r = achs.get(a["id"])
    if r and [records[c]["lore"] for c in r.get("children", [])] != a.get("children", []):
        problems.append(f"achievement {a['id']} children differ")

for k, n in sorted(checked.items()):
    print(f"checked {n:6} {k}")
for p in problems[:60]:
    print("PROBLEM", p)
print(f"{len(problems)} problems")
sys.exit(1 if problems else 0)
