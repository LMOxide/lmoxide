"""World content (smashables, enemies, NPCs, interactables, portals) per playfield, from client
data. Writes data/world_content.json.   Usage: tools/re/py extract_world_content.py

Sources (all local; nothing is downloaded):

* Data/default_windowsx86.demo -- a client demo recording (LMOv1.4, 2015-05) of a real session:
  character creation (4000), the tutorial (1300), Pirate World (4600) and its prison (4610).
  It holds the original server's IIR stream, so for those playfields every spawned character,
  destructible, simple dynel and door is exact ("source": "demo").  Format (DemoMode::LoadEvents,
  ReadEvent): events of u16 type, 8-byte stamp (u32 ?, u32 ms), payload by type: 0/7/8/10-12
  u32 len + bytes; 1/2 u8 stream; 3 (sent) / 4 (received) u8 stream + u32 len + one RPC message
  (u32be hlen, RpcHeader, args); 5 u32; 6 8 bytes; 9 none; 100: 17, 101: 4, 102: 28 bytes.
  Stream 3 is the game server; GameserverToClientRPC::PassBlob carries the IIR records.
* RDB 1000057 PlayfieldSimpleDynels_c (GameData::PlayfieldSimpleDynels_c::ReadData): u32 n, then
  n x { u32 version (4), str name, vec3 pos, quat rot (x,y,z,w), f32 scale, i32 script_group,
  ResourceId visual (u32 type, u32 id: 1010201 = monsterdata look in RDB 1010514, 1010030 =
  plain visual), [v>=2] 6 x f32 bounds, [v>=3] u32 guid (LDB category 55000 key), [v>=4] u8,
  str 4-char tag (JUMP on jump pads) }.  str = u16 len + bytes.  Only the server consumed this
  (no client code calls SimpleDynel::Initialize with it), so these dynels must be spawned by the
  server (SimpleDynelFullUpdateIIR_c, dynel type 51320).
* RDB 1000102 PlayfieldDoors_c: u32 version, u32 n, n x { str name, u32 guid, str tag, u8,
  i32, f32 scale, vec3 pos, quat rot, 6 x f32 bounds, u32 visual type, u32 visual id }.  Door
  dynels (type 51016) are server-spawned too.
* RDB 1000083 PlayfieldTreasures_c: an empty <treasures/> list for every playfield.
* RDB 1000103 static AI points (XML NamedPoint name/position/rotation), 1000090 resurrection
  points (XML), 1000087 teleportdata (XML: portals of every playfield).
* RDB 1010514 monsterdata names, 1000620 item templates (MonsterData stat 359 of destructibles).

* RDB 1000051 playfield path data (navmesh, tools/re/navmesh.py): walkable boxes with their
  ground height.  RDB 1000053 bounded areas (XML; only in the native Linux client, read from
  $LMO_LINUX_CLIENT or ../client-linux): the designers' named regions (ClearHate_*, Res_*,
  ambush, boss encounter and champion areas), also from a world's quest layers (pf + 1, pf + 2).

Playfields without demo data (every world and dungeon in data/destinations.json) get "proposed"
enemies/smashables: themed enemy camps and smashable clusters spread over the navmesh, combat
areas first; dungeons take their parent world's theme (through the portals) one level up, with a
boss at their encounter area and champions where the caves spawned one; friendly NPCs at the
cinematic actor points whose names identify the actor.  Numbers: docs/TUNABLES.md.
"""
import json, os, re, struct, sys
import xml.etree.ElementTree as ET
from collections import defaultdict

import navmesh
from rdb import CLIENT, body, index

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../..")
# The native Linux client (lmo.files.aronwk.com LMO_Linux_v1.5.557666.zip) also ships RDB 1000053.
LINUX_CLIENT = os.environ.get("LMO_LINUX_CLIENT") or os.path.join(ROOT, "../client-linux/LEGO Minifigures Online")
DEMO = os.path.join(CLIENT, "Data/default_windowsx86.demo")
MAIN = [1200, 1300, 4200, 4210, 5200, 5210, 3200, 3210, 2210, 6200, 4600]

idx = index()
LINUX_IDX = index(LINUX_CLIENT) if os.path.isdir(LINUX_CLIENT) else None
DESTINATIONS = {int(k) for k in json.load(open(os.path.join(ROOT, "data/destinations.json")))["destinations"]}


def get(t, i):
    try:
        return body(t, i, idx)
    except (KeyError, FileNotFoundError):
        return None


# ---------------------------------------------------------------- binary helpers

class R:
    def __init__(s, b):
        s.b, s.o = b, 0

    def i32(s):
        s.o += 4
        return struct.unpack_from("<i", s.b, s.o - 4)[0]

    def u32(s):
        s.o += 4
        return struct.unpack_from("<I", s.b, s.o - 4)[0]

    def f(s):
        s.o += 4
        return struct.unpack_from("<f", s.b, s.o - 4)[0]

    def u8(s):
        s.o += 1
        return s.b[s.o - 1]

    def str(s):
        n = struct.unpack_from("<H", s.b, s.o)[0]
        s.o += 2 + n
        return s.b[s.o - n : s.o].decode("latin1")

    def fs(s, n):
        return [s.f() for _ in range(n)]


def rnd(v, n=2):
    return [round(x, n) for x in v]


def yaw(q):
    """Facing (radians around the vertical axis) of an (x, y, z, w) quaternion."""
    import math
    x, y, z, w = q
    return round(math.atan2(2 * (w * y + x * z), 1 - 2 * (y * y + x * x)), 3)


def dist(a, b):
    return ((a[0] - b[0]) ** 2 + (a[2] - b[2]) ** 2) ** 0.5


# ---------------------------------------------------------------- names

def monsterdata_names():
    out = {}
    for (t, r) in idx:
        if t != 1010514:
            continue
        b = get(t, r)
        if not b:
            continue
        parts = b.split(b"\0")
        for i, p in enumerate(parts):
            if p.endswith(b"name") and i + 1 < len(parts):
                out[r] = parts[i + 1].decode("latin1")
                break
    return out


MD = monsterdata_names()


def item_name(t):
    b = get(1000620, t)
    if not b:
        return None
    for m in re.finditer(rb"([\x03-\x60])\x00([A-Za-z][ -~]{2,95})", b):
        n = m.group(1)[0]
        if len(m.group(2)) >= n:
            return b[m.start(2) : m.start(2) + n].decode("latin1")
    return None


def item_monster_data(t):
    """MonsterData (stat 359) of an item template: u32 359, u8, u32 value."""
    b = get(1000620, t)
    i = b.find(struct.pack("<I", 359)) if b else -1
    return struct.unpack_from("<I", b, i + 5)[0] if i >= 0 else None


def destructible_templates():
    """MonsterData look -> destructible item template (dynel type 51321)."""
    out = {}
    for (t, r) in sorted(idx):
        if t != 1000620:
            continue
        b = get(t, r)
        if b and len(b) > 8 and struct.unpack_from("<I", b, 4)[0] == 51321:
            md = item_monster_data(r)
            if md:
                out.setdefault(md, r)
    return out


# ---------------------------------------------------------------- RDB readers

def simple_dynels(pf):
    b = get(1000057, pf)
    if b is None:
        return None
    r = R(b)
    out = []
    for _ in range(r.u32()):
        v = r.u32()
        e = dict(name=r.str().strip(), pos=rnd(r.fs(3)), rot=rnd(r.fs(4), 4), scale=round(r.f(), 3), script_group=r.i32())
        e["visual"] = [r.u32(), r.u32()]
        if v >= 2:
            e["bounds"] = rnd(r.fs(6), 3)
        if v >= 3:
            e["guid"] = r.u32()
        if v >= 4:
            r.u8()
            e["tag"] = r.str()
        out.append(e)
    assert r.o == len(b), (pf, r.o, len(b))
    return out


def doors(pf):
    b = get(1000102, pf)
    if b is None:
        return None
    r = R(b)
    r.u32()
    out = []
    for _ in range(r.u32()):
        e = dict(name=r.str().strip(), guid=r.u32(), tag=r.str())
        r.u8()
        e["script_group"] = r.i32()
        e["scale"] = round(r.f(), 3)
        e["pos"], e["rot"] = rnd(r.fs(3)), rnd(r.fs(4), 4)
        r.fs(6)
        e["visual"] = [r.u32(), r.u32()]
        out.append(e)
    assert r.o == len(b), (pf, r.o, len(b))
    return out


def xml_points(t, pf, tag):
    b = get(t, pf)
    if b is None:
        return None
    out = []
    for e in ET.fromstring(b).iter(tag):
        p = e.find("position")
        if p is None:
            continue
        d = dict(name=e.get("name"), pos=[round(float(p.get(a)), 2) for a in "xyz"])
        q = e.find("rotation")
        if q is not None:
            d["rot"] = yaw([float(q.get(a)) for a in "xyzw"])
        if e.get("Orientation"):
            d["rot"] = round(float(e.get("Orientation")), 3)
        if e.get("Guid"):
            d["guid"] = int(e.get("Guid"))
        out.append(d)
    return out


def portals():
    out = defaultdict(list)
    for p in ET.fromstring(get(1000087, 0)).iter("teleportplayfield"):
        for t in p.iter("teleport"):
            out[int(p.get("SourcePlayfieldID"))].append(
                dict(to=int(t.get("DestinationPlayfieldID")), pos=[round(float(t.get("SourcePos" + a)), 2) for a in "XYZ"]))
    return out


# ---------------------------------------------------------------- classify simple dynels

def look(e):
    t, i = e["visual"]
    return MD.get(i, "") if t == 1010201 else ""


def classify(e):
    n, l = e["name"].lower(), look(e).lower()
    if e.get("tag") == "JUMP" and ("jump" in l or "jump" in n or "volcano" in n) and "target" not in n and "location" not in n:
        return "jump_pad"
    if "jump_pad" in l or (("jump_pad" in n or n.startswith("jumpie") or n.startswith("jumppad")) and not any(k in n for k in ("target", "end", "location", "landing"))):
        return "jump_pad"
    if any(k in n for k in ("target", "landing", "jump location")) or re.search(r"_end$| end$", n):
        return "jump_target"
    if "smashie" in l:
        return "smashable"
    if "buildie" in l:
        return "buildie"
    if "interactable" in l or "lever" in n or "pedestal" in n:
        return "interactable"
    return "marker"


def pair_jumps(items):
    """Jump pad -> landing point ("target", with "target_by").  By name first (jump_pad_X /
    jump_target_X, jumpie_N[_back] / jumpie_N[_back]_target, JumpPad_1_a / JumpPad_1_a_end,
    "Tar 6" / "Tar 6 end", SwampToVolcano1,2 / SwampToVolcanoEnd), else the invisible marker
    listed right after (or before) the pad -- the editor's usual order -- else the nearest one."""
    def key(n, pad):
        n = n.lower().strip()
        n = re.sub(r"jump_?pad_?|jump_target_?|_?target$|_?target_|jump location|[ _]end$", "", n)
        if re.search(r"[a-z]to[a-z]", n):
            n = re.sub(r"\d+$", "", n)
        n = re.sub(r"(_down|_up|_forward)$", "", n)
        return re.sub(r"[\s_]+", "", n)
    def landing(e):
        return e["kind"] == "jump_target" or (e["kind"] == "marker" and e["visual"][1] in (95472, 51057, 193788))
    cands = [j for j, e in enumerate(items) if landing(e)]
    for i, e in enumerate(items):
        if e["kind"] != "jump_pad":
            continue
        k = key(e["name"], True)
        named = [j for j in cands if k and key(items[j]["name"], False) == k]
        adj = [j for j in (i + 1, i - 1) if j in cands and dist(items[j]["pos"], e["pos"]) < 60]
        near = sorted((j for j in cands if dist(items[j]["pos"], e["pos"]) < 40), key=lambda j: dist(items[j]["pos"], e["pos"]))
        for how, c in (("name", named), ("adjacent", adj), ("nearest", near)):
            if c:
                j = min(c, key=lambda j: abs(j - i)) if how != "nearest" else c[0]
                e["target"], e["target_by"] = items[j]["pos"], how
                break


# ---------------------------------------------------------------- the demo recording

def varint(b, o):
    v = s = 0
    while True:
        c = b[o]
        o += 1
        v |= (c & 0x7F) << s
        s += 7
        if c < 0x80:
            return v, o


def pb(b):
    o, out = 0, {}
    while o < len(b):
        k, o = varint(b, o)
        f, w = k >> 3, k & 7
        if w == 0:
            v, o = varint(b, o)
        elif w == 1:
            v, o = b[o : o + 8], o + 8
        elif w == 5:
            v, o = b[o : o + 4], o + 4
        elif w == 2:
            n, o = varint(b, o)
            v, o = b[o : o + n], o + n
        else:
            raise ValueError(w)
        if o > len(b):
            raise ValueError("truncated")
        out.setdefault(f, v)
    return out


def pf32(v):
    return struct.unpack("<f", v)[0]


def pvec(b, n=3):
    d = pb(b)
    return [round(pf32(d[k]), 2) if k in d else 0.0 for k in range(1, n + 1)]


def demo_events(b):
    o, ev = 0, []
    sizes = {5: 4, 6: 8, 9: 0, 0x64: 17, 0x65: 4, 0x66: 28}
    while o + 10 <= len(b):
        t = struct.unpack_from("<H", b, o)[0]
        p, st = o + 10, None
        if t in (0, 7, 8, 10, 11, 12):
            n = struct.unpack_from("<I", b, p)[0]
            pay, p = b[p + 4 : p + 4 + n], p + 4 + n
        elif t in (1, 2):
            st, pay, p = b[p], b"", p + 1
        elif t in (3, 4):
            st, n = b[p], struct.unpack_from("<I", b, p + 1)[0]
            pay, p = b[p + 5 : p + 5 + n], p + 5 + n
        elif t in sizes:
            pay, p = b[p : p + sizes[t]], p + sizes[t]
        else:
            raise ValueError(f"demo event type {t} at {o:#x}")
        ev.append((t, st, pay))
        o = p
    return ev


def java_hash(s):
    h = 0
    for c in s:
        h = (h * 31 + ord(c)) & 0xFFFFFFFF
    return h


IIR = {name: java_hash(name) for name in (
    "PlayfieldFIIR_t", "SimpleCharFullUpdateIIR_t", "SimpleDynelFullUpdateIIR_c", "DestructibleFullUpdateIIR_c",
    "DoorFullUpdateIIR_c", "SpawnClientNPCIIR", "MoveTaskIIR_c")}
KIND = {v: k for k, v in IIR.items()}


def demo_blobs():
    if not os.path.exists(DEMO):
        return []
    g2c = java_hash("GameserverToClientRPC")
    out = []
    for t, st, p in demo_events(open(DEMO, "rb").read()):
        if t != 4 or st != 3:
            continue
        hl = struct.unpack_from(">I", p, 0)[0]
        h = pb(p[4 : 4 + hl])
        ep = pb(h[2])
        if struct.unpack("<I", ep[1])[0] != g2c or h.get(4) != 0:
            continue  # only PassBlob
        n = struct.unpack_from(">I", p, 4 + hl)[0]
        out.append(p[8 + hl : 8 + hl + n])
    return out


GRAVITY = 9.8  # what the original server used for its jump arcs (fits every recorded jump)
DEMO_JUMPS = defaultdict(list)  # playfield -> jumps the original server sent (jump pads)


def demo_jump(b, o):
    """A jump-pad launch: MoveTaskIIR_c (u8 0, vec3 start) carrying a ParameterizedBallisticMoveTask
    (type 12): WaitMoveTask part (u8 0, f32 duration, f32 duration), f32 0, vec3 start, vec3 end,
    vec3 horizontal unit direction, u8 0; then u8 0, f32 horizontal speed, f32 initial vertical
    speed, u8 1, u8 1 (75 bytes).  apex = height of the arc above the higher end point."""
    if len(b) < o + 75 or b[o + 13] != 12 or b[o + 14] != 0:
        return None
    v = struct.unpack_from(">B3fBB2ff3f3f3fBBffBB", b, o)
    dur, start, end, vy = v[6], v[9:12], v[12:15], v[21]
    if not (0.3 < dur < 10) or abs(v[20] * dur - dist(start, end)) > 0.5:
        return None
    apex = vy * vy / (2 * GRAVITY) - max(0.0, end[1] - start[1])
    return dict(start=rnd(start), end=rnd(end), duration=round(dur, 3), apex=round(apex, 2))


def demo_records():
    """Initial state of every dynel the demo's server spawned, per playfield id.  Records are
    found by scanning the PassBlobs for the IIR type ids (records have no length prefix) and
    validated by parsing their protobufs."""
    per = defaultdict(dict)
    pf = None

    def msg(b, o):
        n = struct.unpack_from(">I", b, o)[0]
        if n > len(b) - o - 4:
            raise ValueError
        return pb(b[o + 4 : o + 4 + n]), o + 4 + n

    for blob in demo_blobs():
        for k in range(len(blob) - 13):
            kind = KIND.get(struct.unpack_from(">I", blob, k)[0])
            if not kind or blob[k + 12] > 1:
                continue
            typ, inst = struct.unpack_from(">II", blob, k + 4)
            o = k + 13
            try:
                if kind == "PlayfieldFIIR_t":
                    # ... PlayfieldProxyData (field 2 = model ID32 {type 51100, instance})
                    j = blob.find(b"\x12\x07\x08\x9c\x8f\x03\x10", o, o + 64)
                    if j > 0:
                        pf = varint(blob, j + 7)[0]
                    continue
                if kind == "MoveTaskIIR_c":
                    j = demo_jump(blob, o)
                    if j and pf is not None:
                        DEMO_JUMPS[pf].append(j)
                    continue
                if (typ, inst) in per[pf] or pf is None:
                    continue
                if kind == "SpawnClientNPCIIR":
                    m, _ = msg(blob, o)
                    per[pf][("client", len(per[pf]))] = dict(kind="client_npc", item_template=m.get(2), ai_point=m.get(3, b"").decode(),
                                                            actor_alias=m.get(6, b"").decode(), cinematic=bool(m.get(7)))
                    continue
                h, o = msg(blob, o)
                rec = dict(kind=kind, id=[typ, inst], pos=pvec(h[1]) if 1 in h else None, rot=yaw(pvec(h[2], 4)) if 2 in h else 0.0)
                if kind == "SimpleCharFullUpdateIIR_t":
                    if inst > 16_000_000:
                        continue  # a player
                    for j in range(o, min(o + 400, len(blob) - 9)):
                        if blob[j] != 0x42:  # CoreCharacterFullUpdateIIR 'B', u32, ProtoCoreCharacterIIR
                            continue
                        try:
                            m, _ = msg(blob, j + 5)
                            stt = pb(m[4])
                        except Exception:
                            continue
                        npc = pb(m[5]) if 5 in m else {}
                        rec.update(name=m.get(1, b"").decode("latin1"), monster_data=stt.get(5, 0), level=stt.get(18, 1),
                                   health=stt.get(6, 0), cars_group=stt.get(29, 0),
                                   npc_template=struct.unpack("<I", npc[4])[0] if 4 in npc else None,
                                   npc_type=npc.get(14), npc_flags=npc.get(9))
                        break
                    else:
                        continue
                else:
                    m, o = msg(blob, o)  # ProtoSimpleDynelIIR, then VisualData (u32 id, u32 type, ...)
                    vid, vtype = struct.unpack_from(">II", blob, o)
                    rec.update(name=m.get(2, b"").decode("latin1"), template=m.get(8), cars_group=m.get(5, 0), visual=[vtype, vid],
                               flags=m.get(17), is_hidden=bool(m.get(23)), interaction_radius=m.get(24))
                per[pf][(typ, inst)] = rec
            except (ValueError, KeyError, IndexError, struct.error):
                continue
    return per


# ---------------------------------------------------------------- spells / themes for proposals

DEFAULT_SPELL = 2029  # XX_Axe Chop
SPELLS = [  # (monsterdata name pattern, spell from RDB 1000621)
    ("skeleton", 197335), ("pirate_captain_blackheart", 232073), ("islander_hunter", 194207), ("islander_scout", 194616),
    ("pirate_corsair", 447655), ("pirate_swashbuckler", 230615), ("pirate_desperado", 237323), ("pirate_grenadier", 237324),
    ("monkey_fire", 240436), ("monkey", 240264), ("satyr_shaman", 95024), ("alien_brute", 376387), ("alien_ranger", 376922),
    ("alien_reaper", 379345), ("alien_mindslicer", 377027), ("buzzer", 380316), ("merc_brawler", 372818),
    ("merc_blaster", 373299), ("merc_defender", 373106), ("merc_engineer", 374158), ("bot_artillery", 380345),
    ("dino_raptor", 521074), ("dino_coelophysis", 521074), ("dino_dimetrodon", 520985), ("tiger", 641793),
]


def spell_for(md):
    n = MD.get(md, "")
    return next((s for k, s in SPELLS if k in n), DEFAULT_SPELL)


THEMES = {  # playfield: (level, enemy monsterdata, smashable templates)
    4200: (4, [192826, 192828, 192880, 192821, 207001, 207030, 201478, 201477, 220120], "pirate"),
    4210: (7, [201477, 201479, 201478, 234301, 195925, 236397, 231991, 207030], "pirate"),
    5200: (9, [327844, 327846, 331170, 331167, 350950, 355225], "medieval"),
    5210: (11, [344488, 350094, 337606, 346165, 350740, 327845, 331183, 350305], "medieval"),
    3200: (13, [370897, 383512, 368330, 390750, 377497, 388764], "space"),
    3210: (15, [377497, 379392, 388444, 359212, 386089, 387628, 370898], "space"),
    2210: (17, [82410, 82409, 99330, 76081, 90185, 94792, 98178, 94791, 57036], "myth"),
    6200: (19, [521920, 521911, 521169, 525982, 521311, 527281], "dino"),
}
# destructible templates (dynel type 51321) seen in the demo, by theme
SMASH = {
    "pirate": [153171, 153172, 153146, 149028, 214668, 194305],
    "medieval": [153171, 153146, 194058, 355694, 149028],
    "space": [153172, 194305, 153146, 149028],
    "myth": [153171, 149028, 153146, 194305],
    "dino": [153172, 153146, 194305, 149028],
}
NPC_ACTORS = [  # cinematic actor point name -> friendly NPC monsterdata
    ("Swashbuckler", 436742, "Swashbuckler"), ("VGG", 436747, "Video Game Guy"), ("PizzaGuy", 495904, "Pizza Delivery Man"),
    ("IslandWarrior", 201308, "Island Warrior"), ("Warrior", 201308, "Island Warrior"), ("Revo", 186285, "Revolutionary Soldier"),
    ("PirateDude", 241332, "Pirate"), ("Jester", 327864, "Court Jester"), ("SirLoin", 460064, "Sir Loin"),
    ("Wizard", 460066, "Wizard"), ("Knight", 352446, "Knight"), ("FairyQueen", 352166, "Fairy Queen"),
    ("MeanSoldier", 407116, "Commander"), ("Soldier", 406840, "Space Soldier"), ("Trooper", 406840, "Space Trooper"),
    ("Miner", 407115, "Space Miner"), ("Deino", 485829, "Deino"), ("Enyo", 485830, "Enyo"), ("Pemphredo", 485834, "Pemphredo"),
    ("Mermaid", 34262, "Mermaid"), ("Caveman", 522156, "Caveman"), ("Zookeeper", 522160, "Zookeeper"),
    ("ScubaDiver", 522157, "Deep Sea Diver"), ("DinoTracker", 525532, "Dinosaur Wrangler"), ("Adventurer", 537721, "Paleontologist"),
    ("MonsterScientist", 660086, "Mad Scientist"),
]
NOT_ACTOR = re.compile(r"GoTo|Goto|Go\d|Go$|Move|Camera|Player|Escape|After|Entrance|Frame|Stage|Pos\d|Pos$|Face|Flee|Defeat|"
                       r"Approach|Laugh|Summon|Runaway|Payoff|Briefing|Talk|Ambush|Door|Smash|Bio")
ENEMY_POINT = re.compile(r"AI_Combat|SpawnGuard|SpawnMerc|MercBoss|PirateSpawn|Spawn_|Dreadleg\d*$|Captain\d*$")


NO_COMBAT = {1200, 4620, 30041}  # the hub, the tavern, a developer test map
BOSSES = {  # playfield: [(monsterdata, name, bounded area pattern placing it, else the farthest walkable spot)]
    2300: [(23373, "Medusa", r"Medusa|Res_Boss")],
    2320: [(486592, "Hades", r"BossArea|Res_hades"), (157071, "Charon", r"Res_boatman|ClearHate_Boatman$"),
           (485836, "Deino", r"Res_witches|ClearHate_Witches")],
    2500: [(2254, "Cyclops", r"^Res_Boss$"), (82601, "Harpy Queen", r"Res_HarpyBoss")],
    3300: [(267321, "Evil Mech", r"BossEncounter_Start_Mech|Res_Boss")],
    3310: [(369478, "Alien Queen", r"BossEncounter_Start_AlienQueen|Res_Boss")],
    3500: [(387585, "Alien Saucer", r"BossEncounter_Start_Saucer|^BossEncounter_Start")],
    4300: [(192820, "Captain Pete", r"Res_Boss")],
    4310: [(190679, "Captain Blackheart", r"BossEncounter_Start_Blackheart|res_boss")],
    4500: [(191640, "Crikey the Kraken", r"BossEncounter_Start_CrikeyTheKraken|res_boss")],
    5300: [(327846, "Troll Warlord", r"Res_Boss")],
    5310: [(344513, "Evil Sorcerer", r"Boss")],
    5500: [(316892, "Dragon", r"^BossEncounter_Start")],
    1633: [(416520, "Drake", r"DrakeSpawn")],
    1675: [(636593, "Minotaur", r"Boss")],
    1690: [(57036, "Ogre", r"BA_LastBoss")],
}
# bounded areas (RDB 1000053) where the designers put fights; camps go there first
COMBAT_AREA = re.compile(r"^(ClearHate_|Res_|res_|Harpy_Area|Quest_)|Ambush|Combat|Spawn|Swamp\d|Skeleton|Undead|Ritual|Maze", re.I)
NOT_COMBAT_AREA = re.compile(r"Entrance|_Start$|Teleport|NoLogin|Story|Cine|Head_|Occlusion|FallThrough|Collision|Achievement|Micro", re.I)
CAMP = [(0, 0), (2.2, 1.2), (-1.8, 1.6), (0.6, -2.2), (-2.4, -1.0), (2.6, -1.4)]


def bounded_areas(pf):
    """Named (x, z) polygons with a height band, from the Linux client's RDB 1000053 (the
    Windows client ships only the binary 1000055 form)."""
    if not os.path.isdir(LINUX_CLIENT):
        return []
    try:
        b = body(1000053, pf, LINUX_IDX, LINUX_CLIENT)
    except (KeyError, FileNotFoundError):
        return []
    out = []
    for m in re.finditer(rb"<boundedarea .*?</boundedarea>", b, re.S):
        s = m.group(0).decode("latin1")
        name = re.search(r"<name>\s*(.*?)\s*</name>", s, re.S).group(1)
        h = [float(v) for v in re.search(r"<heights>\s*(.*?)\s*</heights>", s, re.S).group(1).split()]
        poly = [tuple(float(v) for v in vx.split()) for vx in re.findall(r"<vertex>\s*(.*?)\s*</vertex>", s, re.S)]
        if len(poly) >= 3:
            out.append(dict(name=name, heights=h, poly=poly))
    return out


def inside(poly, x, z):
    hit, j = False, len(poly) - 1
    for i in range(len(poly)):
        (xi, zi), (xj, zj) = poly[i], poly[j]
        if (zi > z) != (zj > z) and x < (xj - xi) * (z - zi) / (zj - zi) + xi:
            hit = not hit
        j = i
    return hit


def ground(cells, x, z, near_y=None):
    """Height of the walkable cell under (x, z) (the one nearest near_y if several overlap)."""
    hits = [c for c in cells if c["min"][0] <= x <= c["max"][0] and c["min"][2] <= z <= c["max"][2]]
    if not hits:
        return None
    ys = [c["min"][1] + 0.6 * (c["max"][1] - c["min"][1]) for c in hits]
    return min(ys, key=lambda y: abs(y - near_y)) if near_y is not None else max(ys)


def spread(cands, n, spacing, avoid):
    """Farthest-point sampling: up to n candidates at least `spacing` from each other and from
    `avoid` [(pos, radius)], preferring candidates whose first element (priority) is lower."""
    chosen = []
    for prio in sorted({c[0] for c in cands}):
        pool = [c[1] for c in cands if c[0] == prio]
        while len(chosen) < n and pool:
            def gap(p):
                d = min((dist(p, q) for q in chosen), default=1e9)
                return min([d] + [dist(p, a) - r + spacing for a, r in avoid])
            best = max(pool, key=gap)
            if gap(best) < spacing:
                break
            chosen.append(best)
            pool.remove(best)
    return chosen


def theme_of(pf, parents, seen=()):
    """(level, enemy monsterdata, smashable theme) of a world, or of the world a dungeon is entered from (+1 level)."""
    if pf in THEMES:
        return THEMES[pf]
    for p in sorted(parents.get(pf, ())):
        if p - p % 10 in THEMES:  # a world's quest layer (4212 -> 4210)
            p -= p % 10
        if p not in seen and p != 1200:  # never through the hub, which links to every world
            t = theme_of(p, parents, seen + (pf,))
            if t:
                return t if p in NO_COMBAT else (t[0] + 1, t[1], t[2])
    return None


def enemy(md, level, pos, health_scale=1, name=None, anchor=None):
    return dict(name=name or MD.get(md, str(md)).replace("_", " ").title(), monster_data=md, level=level,
                health=(60 + 15 * level) * health_scale, pos=pos, spell=spell_for(md), anchor=anchor, source="proposed")


def proposals(pf, dest, res, points, portals_here, parents, dungeon):
    """Themed enemy camps and smashable clusters spread over the playfield's walkable cells
    (RDB 1000051), preferring the designers' combat areas (RDB 1000053), away from the arrival
    point, resurrection points and portals; bosses at their encounter areas; friendly NPCs at the
    cinematic actor points whose names identify the actor."""
    out = dict(enemies=[], smashables=[], npcs=[])
    seen = set()
    for p in points or []:
        nm = p["name"]
        if not nm.startswith("Cine_") or NOT_ACTOR.search(nm[5:].replace("StartPos", "")):
            continue
        for k, md, label in NPC_ACTORS:
            if k in nm and label not in seen:
                seen.add(label)
                # level/health/group like the recorded story NPCs (Video Game Guy: L1-3, 93-171 health, CARS 5)
                out["npcs"].append(dict(name=label, monster_data=md, level=1, health=100, cars_group=5, pos=p["pos"], rot=p.get("rot", 0.0),
                                        point=nm, source="cine_point"))
                break
    theme = None if pf in NO_COMBAT else theme_of(pf, parents)
    cells = navmesh.cells(pf, idx)
    arrival = dest["pos"] if dest else (res[0]["pos"] if res else None)
    if not theme or not cells or arrival is None:
        return out
    level, mds, smash = theme
    areas = [a for k in (0, 1, 2) if k == 0 or pf + k not in DESTINATIONS for a in bounded_areas(pf + k)]
    combat = [a for a in areas if COMBAT_AREA.search(a["name"]) and not NOT_COMBAT_AREA.search(a["name"])]
    area = sum((c["max"][0] - c["min"][0]) * (c["max"][2] - c["min"][2]) for c in cells)
    big = [c for c in cells if c["max"][0] - c["min"][0] >= 3 and c["max"][2] - c["min"][2] >= 3]

    def spot(c):
        x, z = (c["min"][0] + c["max"][0]) / 2, (c["min"][2] + c["max"][2]) / 2
        return [round(x, 2), round(c["min"][1] + 0.6 * (c["max"][1] - c["min"][1]), 2), round(z, 2)]

    def area_name(p):
        a = next((a for a in combat if inside(a["poly"], p[0], p[2])), None)
        return a["name"] if a else None

    avoid = [(arrival, 25)] + [(r["pos"], 12) for r in res] + [(q["pos"], 10) for q in portals_here] + [(n["pos"], 8) for n in out["npcs"]]
    # bosses: at their encounter area (its centre, on the ground) or the walkable spot farthest from the entrance
    for md, name, pat in BOSSES.get(pf, []):
        a = next((a for a in areas if re.search(pat, a["name"])), None)
        pos = None
        if a:
            x, z = sum(v[0] for v in a["poly"]) / len(a["poly"]), sum(v[1] for v in a["poly"]) / len(a["poly"])
            y = ground(cells, x, z, a["heights"][0] + 1)
            pos = [round(x, 2), round(y, 2), round(z, 2)] if y is not None else None
        if pos is None:
            pos = spot(max(big or cells, key=lambda c: dist(spot(c), arrival)))
        out["enemies"].append(enemy(md, level + 2, pos, 10, name, a["name"] if a else "farthest"))
        avoid.append((pos, 12))
    # champions where the caves spawned one
    for a in areas:
        if a["name"] == "BA_SpawnChampion":
            x, z = sum(v[0] for v in a["poly"]) / len(a["poly"]), sum(v[1] for v in a["poly"]) / len(a["poly"])
            y = ground(cells, x, z, a["heights"][0] + 1)
            pos = [round(x, 2), round(y, 2), round(z, 2)] if y is not None else spot(min(big or cells, key=lambda c: dist(spot(c), [x, 0, z])))
            out["enemies"].append(enemy(mds[0], level + 1, pos, 4, anchor=a["name"]))
            avoid.append((pos, 10))
    # enemy camps
    n_camps = max(3, min(14, int(area / 250))) if dungeon else min(40, int(area / 800))
    spacing = 14 if dungeon else 26
    cands = [(0 if area_name(spot(c)) else 1, spot(c)) for c in big]
    camps = spread(cands, n_camps, spacing, avoid)
    for i, centre in enumerate(camps):
        size = (2 + i % 2) if dungeon else 3
        kinds = (mds[i % len(mds)], mds[(i + 1) % len(mds)])
        placed = 0
        for dx, dz in CAMP:
            if placed == size:
                break
            x, z = centre[0] + dx, centre[2] + dz
            y = ground(cells, x, z, centre[1])
            if y is None or abs(y - centre[1]) > 2:
                continue
            out["enemies"].append(enemy(kinds[placed % 2], level, [round(x, 2), round(y, 2), round(z, 2)],
                                        anchor=area_name(centre) or "navmesh"))
            placed += 1
    # smashable clusters, away from the camps
    n_smash = max(2, min(8, int(area / 250))) if dungeon else min(30, int(area / 900))
    tpls = SMASH[smash]
    spots = spread([(0, spot(c)) for c in big], n_smash, 12 if dungeon else 22, avoid + [(c, 6) for c in camps])
    n = 0
    for centre in spots:
        for dx, dz in ((0, 0), (0.8, 1.2), (-0.6, 1.4)):
            y = ground(cells, centre[0] + dx, centre[2] + dz, centre[1])
            if y is None or abs(y - centre[1]) > 1.5:
                continue
            t = tpls[n % len(tpls)]
            n += 1
            out["smashables"].append(dict(name=item_name(t) or "Crate", template=t, monster_data=item_monster_data(t), level=1, health=20,
                                          pos=[round(centre[0] + dx, 2), round(y, 2), round(centre[2] + dz, 2)],
                                          anchor=area_name(centre) or "navmesh", source="proposed"))
    if pf == 4210:  # Dreadleg (the pirate boss of Volcano Island) at his cinematic point
        for p in points or []:
            if p["name"].startswith("Cine_Dreadleg"):
                out["enemies"].append(dict(name="Captain Dreadleg", monster_data=634824, level=8, health=1282, pos=p["pos"],
                                           spell=232073, anchor=p["name"], source="proposed"))
                break
    return out


# ---------------------------------------------------------------- main

def main():
    global SMASHIE_TEMPLATE
    SMASHIE_TEMPLATE = destructible_templates()
    dest = json.load(open(os.path.join(ROOT, "data/destinations.json")))["destinations"]
    port = portals()
    demo = demo_records()
    world = {}
    pfs = sorted(set(MAIN) | {p for p in demo if p and p != 4000} | set(port) | DESTINATIONS)
    parents = defaultdict(set)  # playfield -> playfields with a portal into it
    for src, ps in port.items():
        for p in ps:
            if p["to"] != src:
                parents[p["to"]].add(src)
    for pf in pfs:
        w = dict(name=dest.get(str(pf), {}).get("name"), smashables=[], enemies=[], npcs=[], portals=port.get(pf, []))
        sd, dr = simple_dynels(pf), doors(pf)
        res = xml_points(1000090, pf, "resurrectionpoint")
        pts = xml_points(1000103, pf, "NamedPoint")
        if pf not in MAIN and pf not in demo and pf not in DESTINATIONS:
            if w["portals"]:
                world[str(pf)] = dict(portals=w["portals"])
            continue
        items = []
        for e in sd or []:
            e["kind"] = classify(e)
            e["look"] = look(e) or None
            items.append(e)
        pair_jumps(items)
        for e in items:  # where the original server actually launched players from these pads
            if e["kind"] == "jump_pad":
                j = [j for j in DEMO_JUMPS.get(pf, []) if dist(j["start"], e["pos"]) < 1.5]
                if j:
                    e.update(target=j[0]["end"], target_by="demo", apex=j[0]["apex"], duration=j[0]["duration"])
        w["simple_dynels"] = items
        w["doors"] = [dict(d, look=MD.get(d["visual"][1])) for d in dr or []]
        w["resurrection_points"] = res or []
        w["static_points"] = pts or []
        if pf in demo:
            w["source"] = "demo"
            w["demo_dynels"] = []
            w["client_npcs"] = []
            for rec in demo[pf].values():
                k = rec["kind"]
                if k == "client_npc":
                    rec.pop("kind")
                    w["client_npcs"].append(dict(rec, name=item_name(rec["item_template"]) if rec["item_template"] else None))
                elif k == "SimpleCharFullUpdateIIR_t":
                    base = dict(name=rec["name"], monster_data=rec["monster_data"], look=MD.get(rec["monster_data"]), level=rec["level"],
                                health=rec["health"], pos=rec["pos"], rot=rec["rot"], cars_group=rec["cars_group"],
                                npc_template=rec["npc_template"], source="demo")
                    if rec["cars_group"] == 3:
                        w["enemies"].append(dict(base, spell=spell_for(rec["monster_data"])))
                    elif rec["monster_data"] == 51057:  # invisible_monsterdata: cinematic / quest triggers
                        w.setdefault("triggers", []).append(base)
                    else:
                        w["npcs"].append(base)
                elif k == "DestructibleFullUpdateIIR_c":
                    d = dict(name=rec["name"], template=rec["template"], monster_data=rec["visual"][1], look=MD.get(rec["visual"][1]),
                             level=1, health=20, pos=rec["pos"], rot=rec["rot"], cars_group=rec["cars_group"], source="demo")
                    (w["smashables"] if rec["cars_group"] == 3 else w.setdefault("buildies", [])).append(d)
                else:
                    w["demo_dynels"].append(dict(type={"SimpleDynelFullUpdateIIR_c": 51320, "DoorFullUpdateIIR_c": 51016}[k],
                                                 name=rec["name"], template=rec["template"], visual=rec["visual"],
                                                 look=MD.get(rec["visual"][1]) if rec["visual"][0] == 1010201 else None,
                                                 pos=rec["pos"], rot=rec["rot"], cars_group=rec["cars_group"],
                                                 flags=rec["flags"], is_hidden=rec["is_hidden"], interaction_radius=rec["interaction_radius"]))
        else:
            p = proposals(pf, dest.get(str(pf)), res or [], pts, w["portals"], parents, pf not in MAIN)
            w["source"] = "proposed" if p["enemies"] else "rdb"
            for k in ("enemies", "smashables", "npcs"):
                w[k] += p[k]
        # smashables placed in the RDB simple-dynel list (rare: chests)
        for e in items:
            if e["kind"] == "smashable" and e["visual"][1] in SMASHIE_TEMPLATE and not any(dist(e["pos"], s["pos"]) < 0.5 for s in w["smashables"]):
                w["smashables"].append(dict(name=item_name(SMASHIE_TEMPLATE[e["visual"][1]]) or e["look"], template=SMASHIE_TEMPLATE[e["visual"][1]], monster_data=e["visual"][1], level=1, health=20, pos=e["pos"],
                                            rot=yaw(e["rot"]), source="rdb_1000057"))
        world[str(pf)] = w
    world["_comment"] = __doc__.split("\n\n")[0] + " See tools/re/extract_world_content.py for formats and sources."
    path = os.path.join(ROOT, "data/world_content.json")
    with open(path, "w") as f:
        json.dump(world, f, indent=1)
    for pf, w in world.items():
        if isinstance(w, dict) and "simple_dynels" in w:
            kinds = defaultdict(int)
            for e in w["simple_dynels"]:
                kinds[e["kind"]] += 1
            print(f"{pf:>5} {w.get('source', ''):8} enemies {len(w['enemies']):3} npcs {len(w['npcs']):3} smashables {len(w['smashables']):3} "
                  f"portals {len(w['portals']):2} doors {len(w['doors']):2} simple_dynels {dict(kinds)}")
    print("wrote", path)


if __name__ == "__main__":
    main()
