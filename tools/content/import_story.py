#!/usr/bin/env python3
"""Put the client's story data into the content tree (story.rs; rerunnable, it replaces what it
made before; run `cargo run -p lmoxide-content -- fmt` afterwards):

  - tunables.json `story`: the 15 narrated cinematics the server starts (the 16th, the tutorial
    ending, the client starts itself: scriptgroup 635248 Client_CrystalCompass),
  - quests: the quest pickup goals (`counts: pickups`, the client's Lua QuestTriggerProgress
    amounts) and the Dino area quests' reward scripts (Lua 1699/1701/1703/1704),
  - areas: the few story areas only in the client's flagged copy (`<pf>+0x40000000`) that the
    live server used (Story_ReachBeachBackup1 = the live marker of goal 116),
  - triggers (`trigger/<pf>/story-<area>`): cinematics, story beats, talking heads (pusher tags)
    and story goals on the story areas.

Input: research/lmoxide-notes/client-survey (story.md, story_triggers.json, ba_server.json,
story_cinematics.json). Every guess is marked in the record's comment and in docs/TUNABLES.md.

  python3 tools/content/import_story.py [content dir]
"""
import json
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import story_dialogues  # noqa: E402  (the rebuilt conversations; their triggers are made here)

HERE = os.path.dirname(os.path.abspath(__file__))
CONTENT = sys.argv[1] if len(sys.argv) > 1 else os.path.join(HERE, "..", "..", "content")
story_dialogues.CONTENT = CONTENT
SURVEY = os.path.join(HERE, "..", "..", "..", "research", "lmoxide-notes", "client-survey")
ME = "tools/content/import_story.py"


def load(p):
    with open(p) as f:
        return json.load(f)


def save(p, d):
    with open(p, "w") as f:
        json.dump(d, f, indent=2, ensure_ascii=False)
        f.write("\n")


def slug(name):
    return re.sub(r"[^a-z0-9]+", "-", name.lower()).strip("-")


WORLD_DIRS = {d.split("-")[0]: d for d in os.listdir(os.path.join(CONTENT, "worlds"))}
DURATION = {k: v["duration"] for k, v in load(os.path.join(SURVEY, "story_cinematics.json")).items()}

# ------------------------------------------------------------------ cinematics
# name: (effect package, TDB 50000 instance, lore flag, requires, unless, then dialogue, comment)
HUB = "area/1200/story-arriveathub"
CINEMATICS = {
    "game-intro": (635608, 6, None, [], [4081], None,
                   "live: on arriving in the tutorial (CharInPlay), the player in BA_IntroCinematic (certain); "
                   "once: until the tutorial's first beat (Knock Knock 4081)"),
    "pirate-intro": (636984, 8, None, [], [4181], "dialogue/video-game-guy-812",
                     "live: on arriving in Pirate World (CharInPlay, BA_Cine_Arrival), Video Game Guy's "
                     "dialogue 812 38.5 s later; it gives 4181 Arrival at Pirate World (certain)"),
    "after-pirate-fort": (636510, 9, 4241, [4183], [316], None,
                          "guess: arriving at the hub (Story_ArriveAtHub) after the Jacket Fort (4183 Dreadleg "
                          "Escapes Again), before Pirate World is done; sets 4241 'Enter Hub before completed "
                          "Pirate World'"),
    "becoming-a-pirate": (636509, 10, 4286, [], [], None,
                          "likely: dialogue 266's script step BecomeAPirate (resolves 'Complete the Pirate "
                          "Trials'); flag 4286 Cine_Pirate_Defeated_Captain_Pete is a guess"),
    "after-pirate-world": (636511, 11, None, [316], [], "dialogue/medieval-world-news-267",
                           "guess: arriving at the hub once Pirate World is done (316 Captain Dreadleg, its last "
                           "beat; the last step is 'Travel to the LEGO Center')"),
    "medieval-intro": (637080, 12, None, [], [425], "dialogue/the-kings-court-272",
                       "guess: arriving in the Kingdom (the arrival point is in BA_Story_AutoTeleportToThroneRoom1), "
                       "before the King's Court (425)"),
    "after-medieval": (636536, 13, None, [437], [], "dialogue/space-world-news-466",
                       "guess: arriving at the hub once Medieval World is done (437 Another Cube Piece)"),
    "space-intro": (636513, 14, 4285, [], [], "dialogue/space-s02-briefing-327",
                    "likely: BA_Space_Intro (actors at its height), flag 4285 Cine_Space_Intro"),
    "black-hole": (636514, 15, 4281, [], [], None,
                   "likely: BA_Black_Hole_Cinematic by the portal, flag 4281 Cine_Space_BlackHole"),
    "after-space": (636598, 16, None, [492], [], "dialogue/mythology-world-news-530",
                    "guess: arriving at the hub once Space World is done (492, 'End the Alien Invasion')"),
    "mythology-intro": (636983, 17, 4282, [], [], "dialogue/myth-g01-zeus-572",
                        "likely: BA_Medieval_Intro in Mythology Mountains (Zeus and Artemis actors there), flag "
                        "4282 Cine_Mythology_Intro"),
    "after-mythology": (636982, 18, None, [676], [], "dialogue/dino-world-news-633",
                        "guess: arriving at the hub once Mythology World is done (676 XX_G12, 'Disrupt Hades' plan')"),
    "dino-intro": (637077, 19, 4283, [], [], "dialogue/dino-d01-crash-620",
                   "likely: BA_Cine_Intro (actors there), flag 4283 Cine_Dino_Intro"),
    "after-t-rex": (637098, 20, None, [], [], None,
                    "likely: dialogue 690's script step DanceParty (the party's actors at the Gauntlet's end)"),
    "lost-creations-portal": (636512, 21, 4284, [], [], None,
                              "likely: BA_Cine_Lost_Creations, flag 4284 Cine_Dino_Lost_Creations"),
}
# area -> cinematics its trigger names (in order: the first that may play does)
CINEMATIC_AREAS = {
    "area/1300/ba-introcinematic": ["game-intro"],
    "area/4600/ba-cine-arrival": ["pirate-intro"],
    "area/4600/ba-cine-arrival-catcher": ["pirate-intro"],
    HUB: ["after-pirate-fort", "after-pirate-world", "after-medieval", "after-space", "after-mythology"],
    "area/1200/story-arriveathubbackup": ["after-pirate-fort", "after-pirate-world", "after-medieval", "after-space",
                                          "after-mythology"],
    "area/5200/ba-story-autoteleporttothroneroom1": ["medieval-intro"],
    "area/3200/ba-space-intro": ["space-intro"],
    "area/3210/ba-black-hole-cinematic": ["black-hole"],
    "area/2210/ba-medieval-intro": ["mythology-intro"],
    "area/6200/ba-cine-intro": ["dino-intro"],
    "area/6200/ba-cine-lost-creations": ["lost-creations-portal"],
}

# ------------------------------------------------------------------ quest pickups
# goal id: (pickup item templates, % per pickup (Lua), Lua script)
PICKUPS = {
    163: ([464308], 3, 1085),
    232: ([464163, 482089], 4, 1084),
    168: ([464315, 464316, 464317, 464318], 8, 1086),
    259: ([516223, 522494, 522495, 522496], 4, 1533),
    260: ([516222], 2, 1534),
    172: ([398373], 6, 977),
    175: ([397277], 5, 975),
    176: ([397430], 3, 976),
}
# area quest main -> reward script (Lua id, tags, tags only with one of these goals, solves, progress)
REWARD_SCRIPTS = {
    123: (1699, [2401], ["GBWD", "YPFX"], ["YPFX"], [("GBWD", 1)]),
    125: (1701, [1101], [], [], [("GBWD", 1)]),
    124: (1703, [1105], [], [], [("GBWD", 1)]),
    126: (1704, [1102], [], [], [("GBWD", 1)]),
}

# ------------------------------------------------------------------ story goals (Story_* areas)
# area: (goal hash, requires tags, how sure)
GOALS = {
    "area/4600/story-reachbeachbackup1": ("OOPN", [], "certain: the live marker of goal 116 is this area's centre"),
    "area/4600/story-reachedcannon1": ("X7OF", [], "likely: the recording resolved it walking to the cannon"),
    "area/4620/story-reachtavern": ("ATSN", [], "guess (name)"),
    "area/4210/story-reachskullisland1": ("NER2", [], "guess (name)"),
    "area/4210/story-reachpastvolcano1": ("VBVF", [], "guess (name)"),
    "area/2320/story-enteredunderworlddoor1": ("3S33", [], "guess (name)"),
    "area/3210/ba-story-followthepowerline1": ("7XSG", [], "guess (name)"),
    "area/3210/ba-story-jump-into-powerline1": ("MATJ", [], "guess (name)"),
    "area/3210/ba-story-enter-alien-base1": ("3EJ9", [], "guess (name)"),
    "area/3210/ba-story-arrivedtoalienworld1": ("U5AL", [], "guess (name)"),
    "area/3210/ba-story-arrivedtoportal1": ("UC8W", [], "guess (name)"),
    "area/3200/ba-story-exited-space-police-hq": ("CZGR", [], "guess (name)"),
    "area/1200/ba-story-space-found-spaceport1": ("CLH1", [], "guess (name)"),
    "area/1200/ba-story-space-found-castleentrance1": ("WHS5", [], "guess (name)"),
    "area/1200/ba-story-found-mythentrance1": ("FY69", [], "guess (name)"),
    "area/1200/ba-story-found-dinoentrance1": ("6BRI", [], "guess (name)"),
}
# the hub arrival resolves each world's last step, "Travel to the LEGO Center", once that world is done
HUB_GOALS = [("PM25", [316]), ("DN3X", [437]), ("143V", [492]), ("PX18", [676]), ("KPE1", [676])]

# beats: the dialogue text that sets a story area's lore flag (story_triggers.json `match` exact-lore-name
# or number), run without its conversation; left out: the ones run elsewhere (tutorial.rs, cannon.rs,
# knubot.rs arrivals) and those that teleport the player
BEAT_SKIP_PF = {"1300", "1399", "4600", "4610"}
BEAT_SKIP_TEXT = {276}  # the King's Court: teleports out of the throne room
# Beats kept as they are but marked not implemented (`todo` tag and note on the trigger).
BEAT_TODO = {
    181: "story: the beta conversation Escape from the Fort (texts 88-91, 180-191, Captain Pete) is not rebuilt "
         "(user, 2026-10-29: keep it, don't implement it, don't delete it); live played 822 instead (the "
         "Swashbuckler in the jail, recorded), which resolves the same goal ALPQ; only this beat's server side "
         "runs (tag 309, ALPQ)",
}
BEAT_REQUIRES = {
    # the party after T-Rex: its area covers most of the Gauntlet; only after the T-Rex fell
    # (achievement 1540 Roooooar!, granted on the defeat of spawn/6300/t-rex)
    "area/6300/cine-d06-cutscene1": [1540],
    "area/6300/cine-d06-cutscenefallback1": [1540],
}
# recorded dialogues started on a story area (none: the live Island Warrior rescue, 773, starts on
# Monkey Kong's defeat, its record's `defeated`; user, 2026-10-29)
DIALOGUES = {}
for _area, _list in story_dialogues.TRIGGERS.items():
    DIALOGUES.setdefault(_area, []).extend(_list)
# a dialogue's `spawn <hash>` that gives a minifigure (its bag): hash -> (minifigure, evidence)
SPAWN_MINIFIGURES = {
    "7LLT": ("minifig/swashbuckler", "likely: 823 '[Give Swashbuckler]{spawn 7LLT 1 m}' (live capture's escape)"),
    "SIKZ": ("minifig/island-warrior", "likely: 774 '[Island Warrior joins the player]{spawn SIKZ}'"),
    "RG5K": ("minifig/revolutionary-soldier", "likely: 779 '[Revo Soldier joins the player]{spawn RG5K}'"),
    "5WM9": ("minifig/pizza-delivery-man", "likely: 788 '[Pizza Delivery Man joins the player]{spawn 5WM9}'"),
    "8G4X": ("minifig/wizard", "guess: 276 (King's Court) spawns 8G4X; Lua 4048 'XX_give wizard bag tag' gives 4242 "
                               "Received Wizard: the Wizard came as a bag"),
    "GQKS": ("minifig/jester", "guess: 287 after 'You're welcome to come with us, Jester'"),
    "9UO2": ("minifig/battle-goddess", "guess: 555 after Battle Goddess joins ('Come on mortals')"),
    "7WZE": ("minifig/space-miner", "guess: 352 after the Space Miner's 'I'd like to come with you'"),
}

# areas the content lacks: (pf as ba_server.json has it, name) -> role, category. Most are only in the
# flagged copy (pf+0x40000000); the Monkey Cave's rescue area extract_server_areas.py left out
FLAGGED = {
    ("1643", "Cine_Rescue_Island_Warrior1"): ("story", "cinematic"),
    ("4600", "Story_ReachBeachBackup1"): ("story", "cinematic"),
    ("4600", "BA_Head_Diamonds1"): ("story", "talking_head"),
    ("4600", "BA_Head_Minimap1"): ("story", "talking_head"),
    ("4210", "BA_Head_VolcanoDanger1"): ("story", "talking_head"),
    ("3200", "BA_Head_MushroomField1"): ("story", "talking_head"),
    ("3210", "BA_Story_ArrivedToPortal1"): ("story", "cinematic"),
}


def world_dir(pf):
    return os.path.join(CONTENT, "worlds", WORLD_DIRS[pf])


def areas_of(pf):
    p = os.path.join(world_dir(pf), "areas.json")
    return {a["id"]: a for a in load(p)["areas"]} if os.path.exists(p) else {}


def add_flagged_areas():
    ba = load(os.path.join(SURVEY, "ba_server.json"))["areas"]
    for (pf, name), (role, category) in FLAGGED.items():
        rec = next((a for a in ba if a["pf"] == f"{pf}+0x40000000" and a["name"] == name), None)
        flag = "+0x40000000 (the flagged copy)"
        if rec is None:
            rec = next(a for a in ba if a["pf"] == pf and a["name"] == name)
            flag = ""
        p = os.path.join(world_dir(pf), "areas.json")
        d = load(p)
        aid = f"area/{pf}/{slug(name)}"
        d["areas"] = [a for a in d["areas"] if a["id"] != aid]
        d["areas"].append({
            "id": aid, "name": name, "role": role, "category": category,
            "shape": {"polygon": {"points": rec["poly"], "heights": [round(h, 3) for h in rec["heights"]]}},
            "source": "client",
            "comment": f"RDB 1000053:{pf}{flag} guid {rec['guid']} ({ME})",
        })
        save(p, d)


def tunables():
    p = os.path.join(CONTENT, "tunables.json")
    t = load(p)
    cin = {}
    for name, (pkg, text, tag, req, unless, then, comment) in CINEMATICS.items():
        c = {"effect_package": pkg, "text": text, "length": f"{DURATION[str(pkg)]}s"}
        if tag:
            c["tag"] = tag
        if req:
            c["requires"] = req
        if unless:
            c["unless"] = unless
        if then:
            c["then_dialogue"] = then
        c["comment"] = comment
        cin[name] = c
    story = t.setdefault("story", {})
    story["cinematics"] = cin
    story.setdefault("pickup_drop_chance", "100%")
    story["spawn_minifigures"] = {h: m for h, (m, _) in SPAWN_MINIFIGURES.items()}
    save(p, t)


def quests():
    qdir = os.path.join(CONTENT, "quests")
    for f in sorted(os.listdir(qdir)):
        p = os.path.join(qdir, f)
        q = load(p)
        changed = False
        for st in q.get("steps", []):
            for g in st["goals"]:
                if g["goal"] in PICKUPS:
                    items, pct, script = PICKUPS[g["goal"]]
                    g["counts"] = "pickups"
                    g["pickups"] = items
                    # each pickup adds `max / count` (rounded up) = the Lua's amount
                    g["count"] = -(-100 // pct)
                    g["max"] = 100
                    if q.get("kind") == "area":
                        g["dropped_by"] = "kills_or_smashes"
                    else:
                        g.pop("dropped_by", None)
                    changed = True
                if q.get("main") == 122 and g.get("hash") == "GBWD":
                    g["count"] = 4  # the four elemental pieces (Lua 1699/1701/1703/1704 +1 each)
                    changed = True
        if q.get("main") in REWARD_SCRIPTS:
            script, tags, tags_if, solves, progress = REWARD_SCRIPTS[q["main"]]
            rs = {"script": script, "tags": tags}
            if tags_if:
                rs["tags_if_goal"] = tags_if
            if solves:
                rs["solves"] = solves
            # (Lua 1699's ticker buff waits for GBWD: the progress waits for its step)
            rs["progress"] = [dict({"goal": g, "amount": n}, **({"wait": True} if script == 1699 else {})) for g, n in progress]
            q["reward_script"] = rs
            changed = True
        if changed:
            save(p, q)


def trigger(pf, area, name, actions, comment, once="per_player"):
    rec = {"id": f"trigger/{pf}/story-{area.split('/')[-1]}", "name": name, "area": area}
    if once != "never":
        rec["once"] = once
    rec["actions"] = actions
    rec["source"] = "client" if ("certain" in comment and "guess" not in comment) else "guess"
    rec["comment"] = f"{comment} ({ME})"
    return rec


def cmd(name, **args):
    return {"command": {"name": name, "args": args}}


def triggers():
    st = load(os.path.join(SURVEY, "story_triggers.json"))["triggers"]
    out = {}  # pf -> [records]
    names = {}
    for pf in WORLD_DIRS:
        for aid, a in areas_of(pf).items():
            names[aid] = a["name"]

    def add(pf, rec):
        out.setdefault(pf, []).append(rec)

    for area, cins in CINEMATIC_AREAS.items():
        pf = area.split("/")[1]
        comment = "; ".join(f"{c}: {CINEMATICS[c][6]}" for c in cins)
        actions = [{"cinematic": {"name": c}} for c in cins]
        # (the hub: one at a time, each once; the trigger itself every arrival)
        once = "never" if len(cins) > 1 else "per_player"
        if area in (HUB, "area/1200/story-arriveathubbackup"):
            actions += [cmd("story_goal", goal=g, requires=r) for g, r in HUB_GOALS]
            comment += "; the hub arrival resolves each done world's 'Travel to the LEGO Center' (guess)"
        add(pf, trigger(pf, area, names[area], actions, comment, once))

    for area, (hash_, req, how) in GOALS.items():
        pf = area.split("/")[1]
        if area not in names:
            print("no area", area, file=sys.stderr)
            continue
        args = {"goal": hash_}
        if req:
            args["requires"] = req
        add(pf, trigger(pf, area, names[area], [cmd("story_goal", **args)], f"{how}: resolves story goal {hash_}"))

    for area, ds in DIALOGUES.items():
        pf = area.split("/")[1]
        # each dialogue checks its own conditions and plays once (story.rs), so the area may go
        # off every time
        once = "per_player" if len(ds) == 1 and "reconstructed" not in ds[0][1] else "never"
        add(pf, trigger(pf, area, names[area], [{"dialogue": {"dialogue": d}} for d, _ in ds],
                        "; ".join(f"{d.split('/')[1]}: {how}" for d, how in ds), once))
    for area, (text, how) in story_dialogues.BEATS.items():
        pf = area.split("/")[1]
        add(pf, trigger(pf, area, names[area], [cmd("story_beat", text=text)], how))

    for t in st:
        pf, name = t["pf"], t["name"]
        flagged = pf.endswith("+0x40000000")
        pf = pf.replace("+0x40000000", "")
        if pf not in WORLD_DIRS:
            continue
        area = f"area/{pf}/{slug(name)}"
        if area not in names or (flagged and (pf, name) not in FLAGGED):
            continue
        if area in CINEMATIC_AREAS or area in GOALS or area in DIALOGUES or area in story_dialogues.BEATS:
            continue
        if t["kind"] in ("head", "talkinghead") and t.get("tag") and t.get("match"):
            pusher = (t.get("pusher") or {}).get("name") or t.get("tag_name", "").split("/")[-1].strip()
            add(pf, trigger(pf, area, name, [cmd("tag", tag=t["tag"])],
                            f"guess (by name): talking head {pusher}, shown when its tag {t['tag']} is added"))
        elif t["kind"] == "cine" and t.get("match") in ("exact-lore-name", "number") and t.get("dialogue_texts"):
            if pf in BEAT_SKIP_PF:
                continue
            text = int(t["dialogue_texts"][0])
            if text in BEAT_SKIP_TEXT:
                continue
            args = {"text": text}
            if area in BEAT_REQUIRES:
                args["requires"] = BEAT_REQUIRES[area]
            how = "likely" if t["match"] == "exact-lore-name" else "likely (number)"
            rec = trigger(pf, area, name, [cmd("story_beat", **args)],
                          f"{how}: sets {t['tag_name'].split('/')[-1].strip()} ({t['tag']}) with text {text}; "
                          "the conversation before it isn't known, only its server side runs")
            if text in BEAT_TODO:
                rec["tags"] = ["todo"]
                rec["todo"] = BEAT_TODO[text]
            add(pf, rec)

    for pf, recs in out.items():
        # the areas these triggers run are no longer `todo` (extract_server_areas.py drops the mark
        # on rerun too: an area with a trigger on it)
        ap = os.path.join(world_dir(pf), "areas.json")
        ad = load(ap)
        run = {r["area"] for r in recs}
        for a in ad["areas"]:
            if a["id"] in run and "todo" in a.get("tags", []):
                a["tags"].remove("todo")
                if not a["tags"]:
                    del a["tags"]
                a.pop("todo", None)
        save(ap, ad)
        p = os.path.join(world_dir(pf), "triggers.json")
        d = load(p) if os.path.exists(p) else {"triggers": []}
        ids = {r["id"] for r in recs}
        d["triggers"] = [r for r in d["triggers"] if not r["id"].startswith(f"trigger/{pf}/story-") and r["id"] not in ids]
        d["triggers"] += sorted(recs, key=lambda r: r["id"])
        save(p, d)
        print(pf, len(recs), "story triggers")


def jail():
    # BA_TeleportJail (Scry 18602 teleportplayerboundedarea_wazu): through the broken fort wall into
    # the jail (live and the recording: dialogue 211 teleports there first; this is the area's own)
    pf = "4600"
    p = os.path.join(world_dir(pf), "triggers.json")
    d = load(p)
    tid = "trigger/4600/story-ba-teleportjail"
    d["triggers"] = [r for r in d["triggers"] if r["id"] != tid]
    d["triggers"].append({
        "id": tid, "name": "BA_TeleportJail", "area": "area/4600/ba-teleportjail",
        "actions": [{"teleport": {"to": {"world": "world/jail", "point": "TP_Jail"}}}],
        "source": "client",
        "comment": "Scry 18602 teleportplayerboundedarea_wazu: past the broken fort wall into the jail at its "
                   "TP_Jail point (likely; live's dialogue 211 teleports there first, this catches who walks on) "
                   f"({ME})",
    })
    save(p, d)


if __name__ == "__main__":
    story_dialogues.build()
    add_flagged_areas()
    tunables()
    quests()
    triggers()
    jail()
