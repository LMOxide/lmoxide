"""Walls and trigger areas from the client into the content tree (docs/WORLD-CONTENT.md
"Walls and trigger areas"). Rerunnable: it owns only what it wrote before and leaves every other
record (and anything an editor touched: records with an `author`) alone.

Per world (content/worlds/*/world.json of kind hub, pocket, open or instance, playfield < 30000;
a sound layer pf+1 / pf+2 (or one that isn't such a world itself) is read into its parent; a
playable pf+1 / pf+2 such as 1632 Dragoon Dwelling is a world of its own):

  areas.json
    - the server bounded areas (Linux client RDB 1000053 `boundedareas`) not in content yet, by
      name: role `trigger` (with `category` from the name, `scry` = the area's Scry script),
      `kill` (FallThroughWorld), `story` (cinematic, story and talking-head areas: phase 3) or
      `place` (Area_* regions);
    - the server collision walls (1000053 `boundedareas_collision`): role `wall`, blocks by
      `server` (MovementCollisionBehaviour, PathingCollisionEnabled, LOSCombatCollisionBehaviour;
      always an open line: the closed ones repeat their first point);
    - the client's invisible walls (RDB 1040010 `BoundedAreaCollision` layers) of the world's own
      resource: role `wall`, blocks by `client` (closedshape false = an open line,
      StaticCollisionEnabled false = off);
    - territories (RDB 1000050): role `territory` with their roaming paths.
  triggers.json (actions known from client data; docs/TUNABLES.md "Walls and trigger areas")
    - FallThroughWorld: back to the nearest respawn point (teleport command `nearest_respawn`);
    - BA_Speedbuff_*: the speed buff (buff/speed-pad: spell 459025 "XX_Speed Buff") once the
      speed buildie on the pad is built;
    - ClearHate_* (Scry 601883 `ba_clearhatetowardsme`): enemies stop chasing the player.
  spawns.json
    - a speed buildie ("power_up_speed_buildie", the two the live capture recorded on Isle of
      Yarr's pads) on every BA_Speedbuff pad without one: source `proposed`, tag `speed-pad`.
  content/buffs/speed-pad.json from spell 459025.
  Not implemented yet: the `todo` tag and a `todo` note ("<kind>: what is missing") on this run's
  areas that nothing runs (TODO_RULES; docs/WORLD-CONTENT.md "Not implemented yet").

Then run `cargo run -p lmoxide-content -- fmt` and `-- check`.

  tools/re/py extract_server_areas.py [--dry-run]
"""
import json, os, re, struct, sys
from collections import Counter, defaultdict

import gameactions as ga
import item_template as it
import rdb

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "../.."))
CONTENT = os.path.join(ROOT, "content")
LINUX = os.path.join(ROOT, "../clients/linux/LEGO Minifigures Online")
IDX = rdb.index(LINUX)
ME = "extract_server_areas.py"
DRY = "--dry-run" in sys.argv

# name -> category (the order matters: first match), as research/lmoxide-notes/client-survey/scripts/triggers_missing.py
CATS = [
    ("kill_volume", r"^FallThroughWorld"),
    ("teleport", r"^(TP_|BA_TP_|Teleport|BA_Teleport)"),
    ("cinematic", r"^(BA_)?Cine_|Cinematic|^BA_Story|^Story_|^BA_Cine"),
    ("talking_head", r"^(BA_)?Head_"),
    ("hazard", r"Tar|SlowVines|Swamp|Lavabricks|FallingBricks|Piston|fireballs|SpiderJump|Poison|trap|DangerZone|Snowball|CrackedFloor|Leaves|Grill"),
    ("speed_or_jump", r"Speedbuff|jump|Jump|cannon|Cannon|Catapult"),
    ("plate_puzzle", r"Plate|Step\d|Mirror|Light|Brazier|Statues|Bonecage|BoneCage|BuildieTrigger|Whack|Homestone"),
    ("door_wall_shield", r"Door|Barrier|Shield|Collision|Gate"),
    ("spawn_ambush", r"Spawn|Ambush|Champion|NPCAdds|Adds|Combat|Escort|Patrol|Attendants|Runner|Retreat|Raptors|Tentacle|Shamans|Hermit"),
    ("encounter", r"Encounter|Outside|NoLogin|Boss|Arena"),
    ("quest_story", r"Quest|Ask|Tutorial|Achievement|GUI_|Popup|XP|Counter|Story"),
    ("hate_res", r"^(ClearHate|Res_|res_)"),
    ("region", r"^Area_"),
]
ROLE = {"kill_volume": "kill", "cinematic": "story", "talking_head": "story", "region": "place"}

SPEED_TEMPLATE = "object/xx-buildie-level-04-5-buildie-achievement-signs-1-2"
SPEED_NAME = "XX_Buildie : Level 04 : 5 Buildie Achievement : Signs : 1"
SPEED_SPELL = 459025
# the recorded speed buildies stand 1.2-1.3 m above their pad area's bottom (Isle of Yarr)
SPEED_ABOVE_BOTTOM = 1.25
CLEAR_HATE_SCRY = 601883
CLEAR_HATE_FOR = "5s"
STAT_NAMES = {240: "MovementMultiplier"}
PLAYABLE = ("hub", "pocket", "open", "instance")
LAYER_NAME = re.compile(r"sound", re.I)


def slug(name):
    return re.sub(r"[^a-z0-9]+", "-", name.lower()).strip("-") or "area"


def body(t, i):
    try:
        return rdb.body(t, i, IDX, LINUX)
    except (KeyError, FileNotFoundError):
        return None


def tag(s, name):
    m = re.search(rf"<{name}>\s*(.*?)\s*</{name}>", s, re.S)
    return m.group(1) if m else None


def flag(attrs, k, default):
    return attrs.get(k, default) == "true"


def script_names():
    try:
        import rdb_names
        return {int(k): v for k, v in rdb_names.names(1010029, rdb.index()).items()}
    except Exception:
        p = os.path.join(ROOT, "../research/lmoxide-notes/client-survey/rdb_names_1000010.json")
        if os.path.exists(p):
            return {int(k): v for k, v in json.load(open(p)).get("1010029", {}).items()}
    return {}


SCRIPTS = script_names()


def server_areas(res):
    """RDB 1000053 resource `res`: [(section, attrs, name, scry, guid, heights, poly)]."""
    b = body(1000053, res)
    if not b:
        return []
    b = b.decode("latin1")
    out = []
    for sec in re.finditer(r"<(boundedareas(?:_\w+)?)>(.*?)</\1>", b, re.S):
        for m in re.finditer(r"<boundedarea ([^>]*)>(.*?)</boundedarea>", sec.group(2), re.S):
            attrs = dict(re.findall(r'(\w+)="([^"]*)"', m.group(1)))
            s = m.group(2)
            poly = [[round(float(v), 2) for v in x.split()] for x in re.findall(r"<vertex>\s*(.*?)\s*</vertex>", s, re.S)]
            heights = [round(float(v), 3) for v in (tag(s, "heights") or "").split()]
            out.append((sec.group(1), attrs, tag(s, "name") or "", int(tag(s, "scryid") or 0), int(tag(s, "Guid") or 0), heights, poly))
    return out


def client_walls(res):
    """RDB 1040010 resource `res`: the collections with a Collision layer."""
    import xml.etree.ElementTree as ET
    b = body(1040010, res)
    if not b:
        return []
    root = ET.fromstring(b.decode("utf-8").split("?>", 1)[1])
    out = []
    for c in root.findall("boundedareacollection"):
        layers = [L.tag for L in c.find("layers")]
        if "BoundedAreaCollision" not in layers:
            continue
        pts = [[round(float(p.get("x")), 2), round(float(p.get("z")), 2)] for p in c.iter("point")]
        y0 = float(c.get("HeightmapOffset"))
        out.append(dict(name=c.get("name") or "", uid=int(c.get("uniqueid")), attrs=dict(c.attrib), heights=[round(y0, 3), round(y0 + float(c.get("height")), 3)], poly=pts))
    return out


def territories(res):
    b = body(1000050, res)
    if not b:
        return []
    b = b.decode("latin1")
    out = []
    for m in re.finditer(r"<territory>(.*?)</territory>", b, re.S):
        s = m.group(1)
        poly = [[round(float(v), 2) for v in x.split()] for x in re.findall(r"<vertex>\s*(.*?)\s*</vertex>", s, re.S)]
        paths = [[[round(float(v), 2) for v in p.split()] for p in re.findall(r"<point>\s*(.*?)\s*</point>", rp, re.S)]
                 for rp in re.findall(r"<roamingpath>(.*?)</roamingpath>", s, re.S)]
        out.append(dict(name=tag(s, "name") or "territory", category=[tag(s, "categoryname"), tag(s, "categoryvalue")],
                        height=float(tag(s, "Height") or 0), poly=poly, paths=paths))
    return out


def load(path, key):
    if os.path.exists(path):
        return json.load(open(path))
    return {key: []}


def save(path, data, key):
    if DRY:
        return
    if not data[key]:
        if os.path.exists(path):
            os.remove(path)
        return
    with open(path, "w") as f:
        json.dump(data, f, indent=2)
        f.write("\n")


def stable(old, recs):
    """recs in the order their ids had in old (a rerun doesn't reorder the file); new ones last."""
    at = {r["id"]: i for i, r in enumerate(old)}
    return sorted(recs, key=lambda r: at.get(r["id"], len(old)))


def owned_area(a):
    return not a.get("author") and (ME in a.get("comment", ""))


def owned_trigger(t):
    return not t.get("author") and t.get("comment", "").startswith(ME)


def owned_spawn(s):
    return not s.get("author") and "speed-pad" in s.get("tags", []) and s.get("source") == "proposed"


def centre(poly):
    return [sum(p[0] for p in poly) / len(poly), sum(p[1] for p in poly) / len(poly)]


def speed_buff():
    b = rdb.body(1000621, SPEED_SPELL, IDX, LINUX)
    name, _, stats = it.parse(b)
    acts = (ga.lists(SPEED_SPELL, idx=IDX) or {}).get("0") or (ga.lists(SPEED_SPELL, idx=IDX) or {}).get(0) or []
    effects = []
    for a in acts:
        if a["type"] == "Modify":
            st = a["args"]["GameAction_Stat"]
            effects.append({"stat": {"stat": STAT_NAMES.get(st, st), "op": "add", "value": a["args"]["GameAction_Value"]}})
    return {
        "id": "buff/speed-pad",
        "name": "Speed Pad",
        "nano": SPEED_SPELL,
        "duration": f"{stats['TimeExist'] / 100:g}s",
        "effects": effects,
        "source": "client",
        "comment": f"Spell {SPEED_SPELL} \"{name}\" (TimeExist {stats['TimeExist']} cs; its client action Modify does the speed). "
                   f"The BA_Speedbuff pads give it once their speed buildie is built ({ME}; docs/TUNABLES.md).",
    }

# ------------------------------------------------------------------ not implemented yet
# Areas the server doesn't run yet get the `todo` tag and a `todo` note "<kind>: what is missing"
# (docs/WORLD-CONTENT.md "Not implemented yet"): every area of role trigger, story or territory
# without a trigger on it that no server module runs by name, and the client walls the live
# server must have opened. The note's kind is the first rule (world, name pattern, scry) that fits.
TELEPORT_SCRY = 18602
TODO_RULES = [
    # (kind, worlds or None, name pattern, scry or None, note)
    ("teleport", None, r"", TELEPORT_SCRY,
     "Scry 18602 teleportplayerboundedarea_wazu: teleports who walks in; the destination was a script parameter (lost)"),
    ("res", None, r"^(Res_|res_)", None,
     "Scry 174337 boundedarea_discoverresurrectionpoint: discovers the respawn point here (all are known already; "
     "whether live limited respawns to discovered points is unknown)"),
    ("setxp", None, r"FixXP|ResetXP", None, "Scry 276794 ba_setxp: sets the player's XP; the value was a script parameter (lost)"),
    ("escort", None, r"Escort", None,
     "the tutorial escort chain (Scry 597467 genericsendmessagetoself): messages to the escort NPC's script (lost); "
     "tutorial.rs runs the escort without them"),
    ("hazard", None, r"Lavabricks|fireballs", None,
     "lava balls / bricks falling on who walks in (candidate spells 600554 / 600594 \"pirate pf volcano island lava summon "
     "ball / explosion buff\": lava balls at random points 1-7 m around their target); timing and which spell unknown"),
    ("hazard", None, r"^GrillVolumne", None,
     "the Evil Sorcerer's grill fire (candidate spells 318445 / 463750 / 463751 \"evil sorcerer grill1-3 castable\": a 144 s "
     "ground fire, nano 319122, 3 m, AbilityMod 0.25); likely cast in the boss fight, not on entering"),
    ("hazard", None, r"Snowball", None,
     "snowballs rolling at the player (candidate spells 579191 / 580992 \"XX_Snowball\"); the time trial's start, end and "
     "buff removal are unknown"),
    ("hazard", None, r"SpiderJump", None, "spiders jumping down on who walks in; spiders, spell and timing unknown"),
    ("hazard", None, r"CrackedFloor", None, "a floor that breaks under the player; what breaks and where they fall unknown"),
    ("hazard", None, r"FallingBricks", None, "bricks falling on who walks in; spell and timing unknown"),
    ("hazard", None, r"(^|_)Tar|Swamp|SlowVines|SlowField", None,
     "slows (or hurts) who walks in (tar, swamp, vines); the spell is unknown (Scry 176480 boundedarea_castspellonself where "
     "set: its spell parameter is lost)"),
    ("hazard", None, r"Poison|Ritual", None, "poisons who walks in (Scry 176480 boundedarea_castspellonself: the spell parameter is lost)"),
    ("hazard", None, r"trap|DangerZone|Leaves|Piston", None, "a trap set off by walking in; what it does is unknown"),
    ("cannon_catapult", None, r"Cannon|Catapult", None,
     "cannon / catapult shots (training, dodging, siege); what fires, where and how often is unknown (cannon.rs runs only "
     "4600's fort cannon, jumppad.rs the catapult pads)"),
    ("jump", None, r"Jump", None, "jump sounds and fake jump pads around jumps (jumppad.rs runs the pads); what they set off is unknown"),
    ("teleport", None, r"^(TP_|BA_TP_|Teleport|BA_Teleport)", None, "teleports who walks in; the destination is unknown"),
]
TODO_BY_CATEGORY = {
    "spawn_ambush": ("ambush", "spawns enemies when entered; which and how many are unknown (encounter.rs runs only the "
                               "ambush areas extract_world_content.py made)"),
    "plate_puzzle": ("puzzle", "a puzzle step (plates, statues, braziers, whack-a-pirate, bone cages); the rules are unknown "
                               "(plates.rs, mirrors.rs, tiki.rs run the Gauntlet's and Medusa's)"),
    "door_wall_shield": ("door", "a door, shield or barrier switched when entered; the walls they sit on are client walls the "
                                 "client keeps on, or doors encounter.rs / gates.rs open"),
    "encounter": ("encounter", "a boss or event helper area (Outside, NoLogin: leaving or logging in mid-fight); what it did is unknown"),
    "quest_story": ("quest_counter", "a player counter, GUI popup or quest helper; what it counted or showed is unknown"),
    "hate_res": ("res", "a respawn or aggro helper area; what it did is unknown"),
    "hazard": ("hazard", "a hazard; what it does is unknown"),
    "speed_or_jump": ("jump", "a jump or speed helper area; what it did is unknown"),
    "teleport": ("teleport", "teleports who walks in; the destination is unknown"),
    "other": ("other", "purpose unknown (name only)"),
}
# run by name by a server module (no trigger needed)
RUN_BY_MODULE = [
    (None, r"^BA_SpawnChampion"),  # encounter.rs: a side dungeon's champion
    ({6300}, r"^BA_TikiRoom_Plate\d+$"),  # plates.rs
    ({6300}, r"^BA_Piston\d+_\d+$"),  # pistons.rs
    ({1300, 1399, 4600, 4610}, r"^(BA_)?Cine"),  # tutorial.rs, cannon.rs, knubot.rs run the recorded story there
]
# client walls the live server must have opened (the client keeps them on)
WALL_OPENINGS = [
    (r"^Collision_Barrier", "opening: Enchanted Forest barrier (matches the server's BA_FirstBarrier1/2); what opened it "
                            "and how the server tells the client is unknown"),
    (r"^kraken_blocker", "opening: blocks the Kraken bay (Crikey); what opened it and how the server tells the client is unknown"),
    (r"^Mirror_Collision", "opening: the Gauntlet's mirror room walls (movement only); what opened them and how the server "
                           "tells the client is unknown"),
]
STORY_LEFT = {
    "BA_TalkingHead1": "Funhouse talking head: one of the intro pushers 4725 / 4726 / 4728 / 4729, which one per world unknown",
    "BA_Cine_TheKingsCourt1": "the King's Court beat (tag 425, text 276 resolves CPIJ 'Go to the Kingdom', spawns 8G4X) "
                              "teleports out of the throne room: the destination is unknown",
}


def story_survey():
    p = os.path.join(ROOT, "../research/lmoxide-notes/client-survey/story_triggers.json")
    if not os.path.exists(p):
        return {}
    return {(t["pf"], t["name"]): t for t in json.load(open(p))["triggers"]}


STORY = story_survey()


def todo_note(W, a):
    """`kind: note` for an area the server doesn't run yet, or None."""
    name, role = a.get("name", ""), a.get("role")
    if role == "wall":
        return next((f"wall_{n}" for p, n in WALL_OPENINGS if re.search(p, name)), None)
    if role == "territory":
        return "territory: roaming space with paths (RDB 1000050); NPCs don't wander by territories yet"
    if role not in ("trigger", "story"):
        return None
    if any((ws is None or W in ws) and re.search(p, name) for ws, p in RUN_BY_MODULE):
        return None
    if name in STORY_LEFT:
        return f"story: {STORY_LEFT[name]}"
    if role == "story":
        t = STORY.get((str(W), name)) or {}
        if t.get("tag") and t.get("match") == "hand":
            what = f"tag {t['tag']} {(t.get('tag_name') or '').split('/')[-1].strip()}".strip()
            if t.get("dialogue_texts"):
                what += f", text {t['dialogue_texts'][0]}"
            return f"story: hand-matched to {what} (guess); not run"
        if t.get("goal"):
            g = t["goal"]
            return f"story: hand-matched to goal {g.get('hash')} '{g.get('name')}' (guess); not run"
        if t.get("candidates"):
            return f"story: {t['candidates']}; not run"
        return "story: no lore flag, dialogue or goal matched; what it played is unknown"
    for kind, ws, pat, scry, note in TODO_RULES:
        if (ws is None or W in ws) and (scry is None or a.get("scry") == scry) and re.search(pat, name):
            return f"{kind}: {note}"
    kind, note = TODO_BY_CATEGORY.get(a.get("category"), TODO_BY_CATEGORY["other"])
    return f"{kind}: {note}"


def main():
    worlds = {}
    for d in sorted(os.listdir(os.path.join(CONTENT, "worlds"))):
        p = os.path.join(CONTENT, "worlds", d, "world.json")
        if os.path.exists(p):
            w = json.load(open(p))
            worlds[w["playfield"]] = (d, w.get("kind"), w.get("name", ""))
    play = {pf for pf, (_, k, _) in worlds.items() if k in PLAYABLE and pf < 30000}
    # a playable world pf+1 / pf+2 is a world of its own (1632 Dragoon Dwelling next to 1630, 1643
    # Monkey Cave) unless it is a sound layer (1202, 4202, 4212, 5202)
    layers = {pf for pf in play if pf % 10 in (1, 2) and pf - pf % 10 in play and LAYER_NAME.search(worlds[pf][2])}
    play -= layers
    totals = Counter()
    for W in sorted(play):
        d = os.path.join(CONTENT, "worlds", worlds[W][0])
        apath, tpath, spath = (os.path.join(d, f) for f in ("areas.json", "triggers.json", "spawns.json"))
        areas = load(apath, "areas")
        kept = [a for a in areas["areas"] if not owned_area(a)]
        known = {a.get("client_name") or a.get("name") for a in kept} | {a.get("name") for a in kept}
        ids = {a["id"] for a in kept}
        new = []

        def put(rec, base):
            i, n = base, 2
            while i in ids:
                i = f"{base}-{n}"
                n += 1
            rec["id"] = i
            ids.add(i)
            new.append(rec)

        # server areas: the world's own resource, then its layers
        sources = [W] + [W + k for k in (1, 2) if W + k not in play]
        seen = set()
        for res in sources:
            for section, attrs, name, scry, guid, heights, poly in server_areas(res):
                if not name or name in known or (name, section) in seen or len(heights) < 2:
                    continue
                seen.add((name, section))
                where = f"RDB 1000053:{res}" + ("" if section == "boundedareas" else f" {section}")
                script = f"; Scry {scry} {SCRIPTS.get(scry, '?')}" if scry else ""
                rec = {"name": name}
                if section == "boundedareas":
                    if len(poly) < 3:
                        continue
                    cat = next((c for c, p in CATS if re.search(p, name)), "other")
                    rec["role"] = ROLE.get(cat, "trigger")
                    rec["category"] = cat
                else:
                    if len(poly) < 2:
                        continue
                    rec["role"] = "wall"
                    rec["category"] = "collision"
                    rec["blocks"] = {"by": "server", "movement": flag(attrs, "MovementCollisionBehaviour", "true"),
                                     "pathing": flag(attrs, "PathingCollisionEnabled", "false"),
                                     "sight": flag(attrs, "LOSCombatCollisionBehaviour", "true"),
                                     # the server's polygons are lines: the closed ones repeat their first point
                                     # (the others' first and last points lie up to 228 m apart)
                                     "open_line": True}
                if scry:
                    rec["scry"] = scry
                rec["shape"] = {"polygon": {"points": poly, "heights": heights[:2]}}
                rec["source"] = "client"
                rec["comment"] = f"{where} guid {guid}{script} ({ME})"
                put(rec, f"area/{W}/{slug(name)}")
                totals[rec["role"]] += 1
        # the client's own invisible walls
        for c in client_walls(W):
            a = c["attrs"]
            rec = {"name": c["name"] or f"collision {c['uid']}", "role": "wall", "category": "collision",
                   "blocks": {"by": "client", "movement": flag(a, "MovementCollisionBehaviour", "true"),
                              "pathing": flag(a, "PathingCollisionEnabled", "true"),
                              "sight": flag(a, "LOSCombatCollisionBehaviour", "false"),
                              "open_line": a.get("closedshape", "true") == "false",
                              "off": a.get("StaticCollisionEnabled", "true") == "false"},
                   "shape": {"polygon": {"points": c["poly"], "heights": c["heights"]}}, "source": "client",
                   "comment": f"RDB 1040010:{W} uniqueid {c['uid']} ({ME})"}
            if len(c["poly"]) < 2:
                continue
            put(rec, f"area/{W}/wall-{slug(c['name'] or 'collision')}-{c['uid']}")
            totals["client wall"] += 1
        for t in territories(W):
            poly = t["poly"]
            y = min(p[1] for p in poly)
            rec = {"name": t["name"], "role": "territory", "category": f"{t['category'][0] or 'territory'}".lower(),
                   "shape": {"polygon": {"points": [[p[0], p[2]] for p in poly], "heights": [round(y, 3), round(y + t["height"], 3)]}},
                   "paths": t["paths"], "source": "client",
                   "comment": f"RDB 1000050:{W} territory, category {t['category'][0]} = {t['category'][1]} ({ME})"}
            put(rec, f"area/{W}/territory-{slug(t['name'])}")
            totals["territory"] += 1
        areas["areas"] = stable(areas["areas"], kept + new)
        everything = areas["areas"]

        # triggers on them
        trig = load(tpath, "triggers")
        tkept = [t for t in trig["triggers"] if not owned_trigger(t)]
        tids = {t["id"] for t in tkept}
        tnew = []
        spawns = load(spath, "spawns")
        skept = [s for s in spawns["spawns"] if not owned_spawn(s)]
        snew = []
        for a in everything:
            name, shape = a.get("client_name") or a.get("name", ""), (a.get("shape") or {}).get("polygon")
            if not shape:
                continue
            base = f"trigger/{W}/{slug(name)}"
            if base in tids:
                continue
            if name.startswith("FallThroughWorld"):
                t = {"id": base, "name": name, "area": a["id"], "actions": [{"teleport": {"to": {"command": "nearest_respawn"}}}],
                     "source": "client",
                     "comment": f"{ME}: falling out of the world (live's script for it is lost; docs/TUNABLES.md): back at the nearest respawn point"}
            elif name.startswith("BA_Speedbuff"):
                cx, cz = centre(shape["points"])
                bottom = shape["heights"][0]
                near = [s for s in skept if s.get("template") == SPEED_TEMPLATE and ((s["pos"][0] - cx) ** 2 + (s["pos"][2] - cz) ** 2) ** 0.5 < 2.0]
                if near:
                    buildie = near[0]["id"]
                else:
                    buildie = f"spawn/{W}/speed-pad-{slug(name)}"
                    snew.append({"id": buildie, "name": SPEED_NAME, "kind": "buildie", "template": SPEED_TEMPLATE,
                                 "pos": [round(cx, 2), round(bottom + SPEED_ABOVE_BOTTOM, 2), round(cz, 2)], "rot": 0.0,
                                 "source": "proposed", "tags": ["speed-pad"],
                                 "comment": f"The speed buildie of {name}, as the live capture had them on Isle of Yarr's pads ({ME})"})
                t = {"id": base, "name": name, "area": a["id"],
                     "requires": [{"state": {"target": {"placement": buildie}, "state": "built"}}],
                     "actions": [{"apply_buff": {"buff": "buff/speed-pad"}}], "source": "client",
                     "comment": f"{ME}: the speed pad (spell {SPEED_SPELL}) once its speed buildie is built"}
            elif name.startswith("ClearHate_") or a.get("scry") == CLEAR_HATE_SCRY:
                t = {"id": base, "name": name, "area": a["id"], "actions": [{"command": {"name": "clear_hate", "args": {"for": CLEAR_HATE_FOR}}}],
                     "source": "client", "comment": f"{ME}: Scry {CLEAR_HATE_SCRY} ba_clearhatetowardsme: enemies stop chasing whoever walks in"}
            else:
                continue
            tids.add(base)
            tnew.append(t)
            totals["trigger " + t["comment"].split(":")[1].strip()[:12]] += 1
        trig["triggers"] = stable(trig["triggers"], tkept + tnew)
        # not implemented yet: the todo tag and note on this run's areas
        on = {t.get("area") for t in trig["triggers"] if "todo" not in t.get("tags", [])}
        for a in new:
            note = None if a["id"] in on else todo_note(W, a)
            if note:
                a["tags"] = ["todo"]
                a["todo"] = note
                totals["todo " + note.split(":")[0]] += 1
        spawns["spawns"] = stable(spawns["spawns"], skept + snew)
        totals["speed buildie spawns"] += len(snew)
        save(apath, areas, "areas")
        save(tpath, trig, "triggers")
        if snew or any(owned_spawn(s) for s in load(spath, "spawns")["spawns"]):
            save(spath, spawns, "spawns")
        print(f"{W:5} {worlds[W][0]:40} areas +{len(new):4}  triggers +{len(tnew):3}  speed buildies +{len(snew)}")
    buff = speed_buff()
    if not DRY:
        with open(os.path.join(CONTENT, "buffs/speed-pad.json"), "w") as f:
            json.dump(buff, f, indent=2)
            f.write("\n")
    print(dict(totals))
    print(json.dumps(buff))


if __name__ == "__main__":
    main()
