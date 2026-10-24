"""Minifigure combat data for health/damage formulas -> data/minifigure_stats.json.

Per minifigure item template (RDB 1000620, the keys of data/minifigure_spells.json):
  - base stats from the template's stat block (u32 stat, u8 0, u32 value records): MinifigureLife
    3000091, PowerBaseFig 3000014, DefenseBaseFig 3000013, CreativityBaseFig 3000022, role
    3000059, element 3000061, series 3000062;
  - its normal/special click spells, followed through CoreFilteredAreaFormula/ColumnFormula/...
    (GameAction_NanoTemplate), CastNano (GameAction_Value), Projectile (item GameAction_ItemInstance
    -> its CastNano), ColumnDash (TemplateID/Value1) and combo siblings (spells with the same name
    at the next ids) down to the server-side WeaponHit actions, whose AbilityMod (3000021) scales
    the damage;
  - per upgrade ability (data/minifigure_abilities.json): its stat effects (Modify / StatMultiplier
    / ModifyGameAction ...), or the WeaponHits of a 104/105 click override.
Plus the global constants from the game tweaks (RDB 1000516), the player defaults seen in the
recorded original session and the formulas ("model").  Usage: tools/re/py extract_minifig_stats.py"""
import json, os, re, struct

import gameactions as ga
import rdb

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../..")
idx = rdb.index()
STATS = ga.STATS
ITEMS, SPELLS, TWEAKS = 1000620, 1000621, 1000516

BASE_STATS = {3000091: "life", 3000014: "power", 3000013: "defense", 3000022: "creativity",
              3000059: "role", 3000061: "element", 3000062: "series", 3000063: "series_index"}


def body(t, rid):
    try:
        return rdb.body(t, rid, idx)
    except KeyError:
        return None


def actions(rid, t=SPELLS):
    try:
        return ga.actions(rid, t, idx)
    except KeyError:
        return []


def name(b):
    """Last length-prefixed (u16) printable string of a record: the item/spell name."""
    best = None
    for m in re.finditer(rb"([\x03-\x7f])\x00([A-Za-z][ -~]{2,126})", b):
        n = m.group(1)[0]
        if len(m.group(2)) >= n:
            s = b[m.start(2):m.start(2) + n].decode("latin1")
            best = best or s
    return best


def spell_name(rid):
    b = body(SPELLS, rid)
    if not b:
        return None
    return name(b)


def stat_block(b):
    best = []
    for m in re.finditer(rb"\x36\x00\x00\x00\x00", b):  # stat 54 (Level), kind 0, opens the block
        o, recs = m.start(), []
        while o + 9 <= len(b) and b[o + 4] == 0:
            st, v = struct.unpack_from("<IxI", b, o)
            if st == 0 or st > 4_000_000:
                break
            recs.append((st, v))
            o += 9
        if len(recs) > len(best):
            best = recs
    return dict(best)


def tweaks():
    b = body(TWEAKS, 0)
    n, o, out = struct.unpack_from("<I", b, 0)[0], 4, {}
    for _ in range(n):
        ln = struct.unpack_from("<I", b, o)[0]
        k = b[o + 4:o + 4 + ln].decode()
        out[k] = struct.unpack_from("<d", b, o + 4 + ln)[0]
        o += 12 + ln
    return out


def stat_name(s):
    return STATS.get(str(s), str(s))


SKIP_REFS = {"RemoveBuff", "SetSpellRecastTime", "StartEffect", "StopEffect", "LearnSpell", "SetSpellLineRecastTime",
             "LockSpellLineTemplate", "RemoveSpell", "StartParticleEffect", "StopParticleEffect"}


def refs(a):
    """(rdb type, id) of the spells/items an action triggers."""
    t, args = a["type"], a["args"]
    if t in SKIP_REFS:
        return []
    out = [(SPELLS, args[k]) for k in ("GameAction_NanoTemplate", "GameAction_Value1", "GameAction_TemplateID",
                                         "GameAction_TemplateID2") if isinstance(args.get(k), int) and args[k] > 1000]
    if t == "CastNano" and isinstance(args.get("GameAction_Value"), int):
        out.append((SPELLS, args["GameAction_Value"]))
    if isinstance(args.get("GameAction_ItemInstance"), int):
        out.append((ITEMS, args["GameAction_ItemInstance"]))
    return out


def hits(root):
    """WeaponHit/Heal/MultiHit actions reachable from spell `root` (depth-first, with the path)."""
    out, seen = [], set()

    def visit(typ, rid, path, depth):
        if (typ, rid) in seen or depth > 8 or not rid:
            return
        seen.add((typ, rid))
        for a in actions(rid, typ):
            t, args = a["type"], a["args"]
            if t in ("WeaponHit", "Heal", "MultiHit"):
                out.append({"spell": rid, "type": t, "ability_mod": round(args.get("AbilityMod", 1.0), 4),
                            "damage_type": args.get("GameAction_DamageType"), "via": path})
                continue
            for r in refs(a):
                visit(r[0], r[1], path + [rid], depth + 1)

    visit(SPELLS, root, [], 0)
    # combo chain: the next spells with the same name (e.g. Gladius Slash 317739 -> 317740 -> 317741)
    nm = spell_name(root)
    for nxt in range(root + 1, root + 4):
        if nm and spell_name(nxt) == nm:
            visit(SPELLS, nxt, [nxt], 0)
        else:
            break
    return out


EFFECT_TYPES = {"Modify", "Set", "TempChangeStat", "StatMultiplier", "ModifyGameAction", "AddBuff",
                "AddBuffCounter", "AddProc", "AddSpellProc", "RegisterSubSpellForSpellLine", "CastNano"}


def ability(aid, slot):
    acts = actions(aid)
    effects = []
    for a in acts:
        if a["type"] not in EFFECT_TYPES:
            continue
        args = {k: (round(v, 4) if isinstance(v, float) else v) for k, v in a["args"].items()}
        e = {"type": a["type"]}
        if "GameAction_Stat" in args:
            e["stat"] = args.pop("GameAction_Stat")
            e["stat_name"] = stat_name(e["stat"])
        if "GameAction_Value1" in args and a["type"] == "ModifyGameAction":
            e["stat"] = args["GameAction_Value1"]
            e["stat_name"] = stat_name(e["stat"])
        args.pop("GameAction_ApplyOn", None)
        e.update(args)
        effects.append(e)
    out = {"ability": aid, "name": spell_name(aid), "slot": slot, "effects": effects}
    if slot in (104, 105):
        out["hits"] = hits(aid)
    elif not effects:
        out["note"] = "no client-side game actions (proc/behaviour is server-only)"
    return out


def main():
    spells = json.load(open(os.path.join(ROOT, "data/minifigure_spells.json")))
    abilities = json.load(open(os.path.join(ROOT, "data/minifigure_abilities.json")))
    tw = tweaks()
    figures = {}
    for t in sorted(spells, key=int):
        b = body(ITEMS, int(t))
        if not b:
            continue
        st = stat_block(b)
        fig = {"name": name(b)}
        fig.update({k: st.get(s, 0) for s, k in BASE_STATS.items()})
        normal, special = spells[t]
        fig["normal"] = {"spell": normal, "name": spell_name(normal), "hits": hits(normal)}
        fig["special"] = {"spell": special, "name": spell_name(special), "hits": hits(special)}
        fig["upgrades"] = [dict(bit=pos, **ability(aid, slot)) for pos, aid, slot in abilities.get(t, [])]
        figures[t] = fig
    model = {
        "source": "client: SimpleChar_t::CalculateLife, AttackUtils_c::CalculatePowerRating/CalculateRawDamage; "
                  "tweaks RDB 1000516; player defaults from the recorded original session",
        "tweaks": {k: tw[k] for k in sorted(tw) if k.startswith(("AttackUtils_", "TWK_MaxPlayerLevel", "XPLevelRatio"))},
        "player_defaults": {
            "HealthPerLevel": 100, "HealthBase": 250, "PowerRatingAll": 50, "DefenseRatingAll": 50,
            "CreativityRatingAll": 50, "DefenseRatingTarget": 1550, "PowerEffectMin": 1.0, "PowerEffectPerLevel": 0.0,
            "DefenseEffectMin": 1.0, "CreativityEffectMin": 1.0, "FinalDamageMultiplier": 1.0,
            "FinalDefenseMultiplier": 1.0, "MaxMitigation": 0.55, "HealthRegenOOC": 0.05, "HealthRegenIC": 0.005},
        "formulas": {
            "life": "floor(((Level*HealthPerLevel + MinifigureLife) + LifeAdjust) * LifeMultiplier + 0.5); "
                    "LifeMultiplier = prod(1 + StatMultiplier%/100)",
            "power_rating": "(int(PowerBaseFig*Level*MinifigureStarFactor) + PowerRatingAll + PowerRating[dt]) "
                            "* (PowerEffectMin + MinifigureLevel*PowerEffectPerLevel) * GlobalPowerModifier; "
                            "dt 1000..1004 -> PowerRating1..4,5",
            "raw_damage": "power_rating * AbilityMod * FinalDamageMultiplier * (1 + V*(2r-1)), r in [0,1), "
                          "V = AttackUtils_DamageFlagVariance",
        },
    }
    out = {"model": model, "minifigures": figures}
    json.dump(out, open(os.path.join(ROOT, "data/minifigure_stats.json"), "w"), indent=1)
    nh = sum(1 for f in figures.values() if f["normal"]["hits"])
    print(len(figures), "minifigures;", nh, "with normal-attack WeaponHits")


if __name__ == "__main__":
    main()
