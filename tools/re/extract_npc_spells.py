"""Enemy abilities: which spells each enemy monster casts, and the spells' game action lists,
for the server's enemy AI (crates/lmoxide-server/src/npc_spells.rs). Writes data/npc_spells.json:

  monsters: monsterdata -> {name, spells: [spell], source}
  spells:   spell -> {name, stats: {AttackRange, RechargeDelay, ItemDelay, ChannelingInterval,
            ChannelingTicks, TargetingMode, TimeExist, AISpellCategory, AIPowerrating,
            EffectPackage}, lists: {event: [{type, client, args, criteria}]}}   (as spell_actions.json)
  items:    projectile item -> {name, actions: [...], effect_package}   (what a shot does on impact)

Where the abilities come from. NPC templates (and with them each monster's spell list) were
server-side: no client resource references the spells the recorded enemies cast (a scan of every
RDB resource for their ids finds only noise), and monsterdata (RDB 1010514) holds only the look.
The client does have the spells themselves (RDB 1000621): "XX_<monster> <attack> castable" and
"XX_boss <name> ..." with AI stats (AISpellCategory, AIPowerrating), plus generic "XX_Slash",
"XX_Shot", ... ones. The monster -> spells table is therefore built from:

  recorded  the spells the original server cast for the recorded session's enemies
            (Data/default_windowsx86.demo, BehaviorControlCenterIIR_c action 4 MagicCommands):
            Scallywags (skeleton_pirate_1: XX_Shot 197370 + XX_Slash 197365; skeleton_pirate_2:
            XX_Shot 197338), Musketeers (XX_Shot 191502), Captain Dreadleg (slash 232292, spin 238132)
  named     spells whose name names the monster's family (FAMILIES below, matched by hand; the
            matches are guesses, see docs/TUNABLES.md)
  generic   the rest: generic enemy spells (XX_Arrow Shot, XX_Bite, ...) by what the monster is
            (GENERIC below, guesses)

Monsters with neither keep the spawn's own spell (data/world_content.json `spell`, by default
XX_Axe Chop). Spells only kept when they have an attack AI category (or none) and aren't spawn
controls, auras or on-spawn/combat/victory triggers.  Usage: tools/re/py extract_npc_spells.py
"""
import json, os, re

import gameactions as ga
import item_template as it
import rdb

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../..")
idx = rdb.index()

KEEP = ("AttackRange", "RechargeDelay", "ItemDelay", "ChannelingInterval", "ChannelingTicks", "TargetingMode", "TimeExist",
        "AISpellCategory", "AIPowerrating", "EffectPackage")

# monsterdata -> spells the original server cast for it in the recorded session
RECORDED = {
    207001: [197370, 197365],  # skeleton_pirate_1 (Scallywag, tutorial): recs 958-1074
    207030: [197338],  # skeleton_pirate_2 (Scallywag, tutorial): recs 645-648
    192826: [191502],  # pirate_musketeer (Musketeer, 4600): recs 3291-3371
    634824: [232292, 238132],  # pirate_captain_blackheart_no_death (Captain Dreadleg, tutorial): recs 1686-2101
}

# monsterdata name (regex, whole name) -> spell names (exact, case-insensitive)
FAMILIES = [
    (r"pirate_captain_blackheart", ["XX_boss dreadleg slash castable", "XX_boss dreadleg spin castable",
                                    "XX_boss dreadleg shot castable", "XX_boss dreadleg bomb castable"]),
    (r"pirate_corsair", ["XX_pirate corsair spin castable"]),
    (r"pirate_islander_scout", ["XX_islander scout trap castable"]),
    (r"satyr_shaman", ["XX_satyr shaman fire ranged castable"]),
    (r"space_alien_brute_[ab]", ["XX_alien brute shot castable"]),
    (r"space_alien_ranger", ["XX_alien ranger shot castable"]),
    (r"space_alien_reaper", ["XX_alien reaper slash castable"]),
    (r"space_alien_mindslicer", ["XX_alien mindslicer bolt castable"]),
    (r"space_buzzer", ["XX_alien buzzers slash castable"]),
    (r"space_merc_brawler", ["XX_merc brawler slash castable"]),
    (r"space_merc_blaster", ["XX_merc blaster shoot castable"]),
    (r"space_merc_defender", ["XX_merc defender shot castable"]),
    (r"space_merc_engineer", ["XX_merc engineer shot castable"]),
    (r"space_mushroom_explode", ["XX_mushroom bulb explode castable"]),
    (r"space_mushroom_poison", ["XX_mushroom bulb slime pulse castable"]),
    (r"dino_raptor", ["XX_dino raptor small melee hit castable"]),
    (r"dino_raptor_fire", ["XX_dino raptor small ranged shot castable"]),
    (r"drake", ["XX_drake razor breath castable"]),
    (r"ogre", ["XX_myth pd armory champ ogre lightningblast melee castable",
               "XX_myth pd armory champ ogre lightningblast projectile castable"]),
    (r"medieval_evil_sorcerer", ["XX_medieval pd castle evil sorcerer whack castable",
                                 "XX_medieval pd castle evil sorcerer portal castable"]),
    (r"medusa", ["XX_boss medusa basic castable", "XX_boss medusa special castable"]),
    (r"hades", ["XX_boss hades attackleft normal castable", "XX_boss hades attackright normal castable",
                "XX_boss hades attackmiddle normal castable", "XX_boss hades bolt castable", "XX_boss hades charge castable",
                "XX_boss hades groundfire castable"]),
    (r"kraken", ["XX_boss kraken tentacle swipe left castable", "XX_boss kraken tentacle swipe right castable",
                 "XX_boss kraken ink bubble castable", "XX_boss kraken tentacle smash single castable",
                 "XX_boss kraken tentacle smash tripple castable"]),
    (r"space_alien_saucer", ["XX_space dg alienhq boss saucer bolt castable", "XX_space dg alienhq boss saucer ball castable",
                             "XX_space dg alienhq boss saucer spin castable"]),
    (r"evil_mech", ["XX_space pd policeHQ boss mech stomp castable", "XX_space pd policeHQ boss mech minigun lvl1 castable"]),
    (r"space_alien_queen", ["XX_space pd port boss queen blast castable", "XX_space pd port boss queen purge castable"]),
]

# Monsters without spells of their own in the client data get generic enemy spells by what they
# are (archer: an arrow, hound: a bite, troll heavy hitter: an area smash, ...): spell ids, since
# most generic names have several variants (AISpellCategory 512 melee, 1024 ranged). Guesses
# (docs/TUNABLES.md); dashes, stuns, heals and self-buffs left out (npc_spells.rs doesn't run them).
GENERIC = [
    (r"troll_heavy_hitter_.*", [319946, 100799]),  # XX_Smash (area), XX_Rampage
    (r"troll_melee_.*", [238037, 196247]),  # XX_Smash, XX_Wild Swing
    (r"troll_ranged_.*", [90650]),  # XX_Spear Throw
    (r"troll_hornblower_.*", [196229]),  # XX_Smack
    (r"troll_shaman_.*", [319424]),  # XX_Bolt
    (r"skeleton_archer", [319299]),  # XX_Arrow Shot
    (r"skeleton_trident", [194198, 99055]),  # XX_Stab, XX_Spear Throw
    (r"skeleton_axe", [196247, 319341]),  # XX_Wild Swing, XX_Smash
    (r"skeleton_ancient_attacker", [319786]),  # XX_Strike
    (r"pirate_swashbuckler", [194140]),  # XX_Slash
    # XX_Bomb throws the pirate_grenadier_bomb (item 190832, effect 202159); XX_Grenade Throw 238116
    # guessed before lobs item 238118, which looks like the Pirate Medic's heal bottle (the user)
    (r"pirate_grenadier", [190796, 238115]),  # XX_Bomb, XX_Slash
    # the live capture's Hunters (201477) cast only XX_Bolas and XX_Stab; XX_Dart was the Scouts'
    (r"pirate_islander_hunter", [194199, 194198]),  # XX_Bolas, XX_Stab
    # his tiki totem is tunables enemy_abilities.totems; the lobbed XX_Magic Potion 95237 guessed
    # before looked like a snowball; he had a knife attack, used more often (the user, 2026-10-30)
    (r"pirate_islander_witch_doctor", [194198]),  # XX_Stab
    (r"pirate_captain_pete", [201244, 238115]),  # XX_Cannon, XX_Slash
    (r"harpy(_blade)?", [15086, 196231]),  # XX_Feather Fling, XX_Smack
    (r"gremlin_warlock_.*", [332423]),  # XX_Bolt
    (r"gremlin_melee_.*", [196232]),  # XX_Smack
    (r"medieval_ogre_defender_.*", [319338]),  # XX_Smash (area)
    (r"satyr", [194198]),  # XX_Stab
    (r"bot_battle_merc", [331628]),  # XX_Shot
    (r"(space_)?mushroom_runner", [321437]),  # XX_Explode
    (r"medieval_wolf", [16203]),  # XX_Bite
    (r"monkey", [196252]),  # XX_Banana
    (r"monkey_fire", [236405]),  # XX_Tiki Fire Breath
    (r"dino_coelophysis", [18249]),  # XX_Bite
    (r"dino_dimetrodon", [16208, 16203]),  # XX_Fire Breath, XX_Bite
    (r"dino_pterodactyl", [196231]),  # XX_Smack
    (r"lizard_man", [99058, 194198]),  # XX_Spear Throw, XX_Stab
    (r"treant_.*", [101016, 16101]),  # XX_Smash (area), XX_Sweep
    (r"hound", [16203]),  # XX_Bite
    (r"hound_fire", [16208, 16203]),  # XX_Fire Breath, XX_Bite
    (r"medieval_leafscreamer_.*", [342476]),  # XX_Venom
    (r"gorgon", [100144, 194140]),  # XX_Snake Shot, XX_Slash
    (r"medieval_harlequin_minion_.*", [196233]),  # XX_Smack
    (r"spider_.*", [342476, 342475]),  # XX_Venom, XX_Stab
    (r"minotaur_monster_boss", [319891, 319338]),  # XX_Rampage, XX_Smash
    (r"charon", [196247, 332423]),  # XX_Wild Swing, XX_Bolt
    (r"mythology_witch_deino_combat", [330144]),  # XX_Bolt
    (r"Cyclops", [319946, 16199]),  # XX_Smash (area), XX_Rock Chuck
    (r"dragon", [321687, 101016]),  # XX_Dragon's Breath, XX_Smash
]

# AISpellCategory bits that mark a spell as something other than an attack (on-spawn 128,
# on-combat 256, taunt 2048, on-victory 4096, self buffs 8192, environmental 16384)
NOT_ATTACK = 128 | 256 | 2048 | 4096 | 8192 | 16384
FOLLOW = ("CastNano", "CoreFilteredAreaFormula", "ColumnFormula", "CoreRandomPointAreaFormula", "CorePersistentGroundEffect",
          "PlaceTrap", "RemoveBuff")


def short(args):
    return {k.replace("GameAction_", ""): v for k, v in args.items()}


def spell_record(spell):
    try:
        b = rdb.body(1000621, spell, idx)
    except (KeyError, FileNotFoundError):
        return None
    name, _desc, stats = it.parse(b)
    lists = ga.lists(spell, idx=idx) or {}
    return {
        "name": name,
        "stats": {k: v for k, v in stats.items() if k in KEEP},
        "lists": {
            str(e): [
                {"type": a["type"], "client": a["client"], "args": short(a["args"]), **({"criteria": a["criteria"]} if a["criteria"] else {})}
                for a in acts
            ]
            for e, acts in sorted(lists.items())
        },
    }


def item_record(item):
    try:
        d = it.dump(item, idx)
    except (KeyError, FileNotFoundError):
        return None
    return {
        "name": d["name"],
        "actions": [{"type": a["type"], "client": a["client"], "args": short(a["args"])} for a in d["actions"]],
        "effect_package": d["stats"].get("EffectPackage"),
    }


def monsterdata_names():
    out = {}
    for (t, r) in idx:
        if t != 1010514:
            continue
        try:
            parts = rdb.body(t, r, idx).split(b"\0")
        except (KeyError, FileNotFoundError):
            continue
        for i, p in enumerate(parts):
            if p.endswith(b"name") and i + 1 < len(parts):
                out[r] = parts[i + 1].decode("latin1")
                break
    return out


def spells_by_name():
    out = {}
    for (t, r) in sorted(idx):
        if t != 1000621:
            continue
        try:
            name, _d, stats = it.parse(rdb.body(t, r, idx))
        except (KeyError, FileNotFoundError):
            continue
        if name:
            out.setdefault(name.lower(), []).append((r, stats))
    return out


def is_attack(stats):
    return not (stats.get("AISpellCategory", 0) & NOT_ATTACK)


def main():
    md_names = monsterdata_names()
    by_name = spells_by_name()
    # every enemy the server spawns
    enemies = set()
    worlds = json.load(open(os.path.join(ROOT, "data/archive/world_content.json")))
    for w in worlds.values():
        if isinstance(w, dict):
            enemies |= {e["monster_data"] for e in w.get("enemies", [])}
    for defs in json.load(open(os.path.join(ROOT, "data/archive/spawns.json"))).values():
        enemies |= {d["monster_data"] for d in defs if d.get("kind", "enemy") == "enemy"}

    monsters = {}
    for md in sorted(enemies):
        name = md_names.get(md, "")
        if md in RECORDED:
            monsters[str(md)] = {"name": name, "spells": RECORDED[md], "source": "recorded"}
            continue
        spells = []
        for pattern, names in FAMILIES:
            if re.fullmatch(pattern, name):
                for n in names:
                    cands = [(r, st) for r, st in by_name.get(n.lower(), []) if is_attack(st)]
                    # several spells of one name: the one with AI stats and a range
                    cands.sort(key=lambda c: ("AISpellCategory" not in c[1], "AttackRange" not in c[1], c[0]))
                    if cands:
                        spells.append(cands[0][0])
                    else:
                        print("no spell named", n)
        if spells:
            monsters[str(md)] = {"name": name, "spells": list(dict.fromkeys(spells)), "source": "named"}
            continue
        for pattern, ids in GENERIC:
            if re.fullmatch(pattern, name):
                spells += [i for i in ids if spell_record(i)]
        if spells:
            monsters[str(md)] = {"name": name, "spells": list(dict.fromkeys(spells)), "source": "generic"}

    # the spells, every nano they reach and the projectile items they shoot
    spells, items = {}, {}
    todo = [s for m in monsters.values() for s in m["spells"]]
    while todo:
        s = todo.pop()
        if str(s) in spells:
            continue
        rec = spell_record(s)
        if not rec:
            continue
        spells[str(s)] = rec
        for acts in rec["lists"].values():
            for a in acts:
                g = a["args"]
                if a["type"] in FOLLOW:
                    # (TemplateID2: a ground effect's nano, Captain Dreadleg's bomb aoe 238199)
                    todo += [g[k] for k in ("Value", "NanoTemplate", "TemplateID", "TemplateID2") if isinstance(g.get(k), int) and g[k] > 1000]
                if a["type"] == "Projectile" and g.get("ItemInstance") and str(g["ItemInstance"]) not in items:
                    irec = item_record(g["ItemInstance"])
                    if irec:
                        items[str(g["ItemInstance"])] = irec
                        for ia in irec["actions"]:
                            ig = ia["args"]
                            todo += [ig[k] for k in ("Value", "NanoTemplate") if isinstance(ig.get(k), int) and ig[k] > 1000]

    out = {
        "_comment": "Enemy abilities (tools/re/extract_npc_spells.py): monsterdata -> spells (recorded or matched by name), "
                    "the spells' game action lists and the projectile items they shoot.",
        "monsters": monsters,
        "spells": dict(sorted(spells.items(), key=lambda kv: int(kv[0]))),
        "items": dict(sorted(items.items(), key=lambda kv: int(kv[0]))),
    }
    with open(os.path.join(ROOT, "data/archive/npc_spells.json"), "w") as f:
        json.dump(out, f, indent=1, sort_keys=False)
        f.write("\n")
    print(len(monsters), "monsters,", len(spells), "spells,", len(items), "projectile items")
    for md, m in monsters.items():
        print(" ", md, m["name"], m["source"], [f"{s} {spells.get(str(s), {}).get('name')}" for s in m["spells"]])


if __name__ == "__main__":
    main()
