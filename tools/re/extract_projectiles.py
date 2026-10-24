"""Projectile parameters of every minifigure spell (left/right-click spells, upgrade abilities):
data/projectiles.json, spell -> {item, type, speed, attractor, offset_z, ricochets, range,
charge_items, channel_interval, channel_buff}. Charge spells (hold to grow, release to fire: e.g. Big Blast)
have one Projectile action per charge level (OnChannelingEnd list, gated on a buff counter);
charge_items lists their items by level, channel_interval (s) is ChannelingInterval (stat 447) and
channel_buff the buff shown on the caster while charging (re-applied each interval).
The client's Projectile GameAction does nothing by itself; the server spawns the projectile dynel
(CoreProjectileFullUpdateIIR) from these values.  Usage: tools/re/py extract_projectiles.py"""
import json, os, struct

import rdb
from gameactions import actions

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../..")
spells = json.load(open(os.path.join(ROOT, "data/minifigure_spells.json")))
abilities = json.load(open(os.path.join(ROOT, "data/minifigure_abilities.json")))
ids = {s for v in spells.values() for s in v} | {a[1] for v in abilities.values() for a in v}
idx = rdb.index()
out = {}
for sid in sorted(ids):
    try:
        acts = actions(sid, idx=idx)
    except Exception:
        continue
    projectiles = [a for a in acts if a["type"] == "Projectile"]
    for a in projectiles[:1]:
        g = a["args"]
        out[str(sid)] = {
            "item": g.get("GameAction_ItemInstance", 0),
            "type": g.get("GameAction_ProjectileType", 1),
            "speed": g.get("GameAction_Speed", 1000) / 100,
            "attractor": g.get("AttractorID", g.get("GameAction_AttractorID", 0)),
            "offset_z": g.get("GameAction_DisplaceMentZ", 0) / 100,
            "ricochets": g.get("GameAction_MaxRicochets", 0),
            "range": g.get("AttackRange", 0) / 100,
        }
        if len(projectiles) > 1:
            body = rdb.body(1000621, sid, idx)
            at = body.find(struct.pack("<I", 447) + b"\x00")  # ChannelingInterval stat (u32, u8, u32)
            interval = struct.unpack_from("<I", body, at + 5)[0] / 100 if at >= 0 else 0.5
            out[str(sid)]["charge_items"] = [p["args"].get("GameAction_ItemInstance", 0) for p in projectiles]
            out[str(sid)]["channel_interval"] = interval
            # the channel's visual: the buff the Use list's CastNano applies to the caster
            nano = next((a["args"].get("GameAction_Value") or a["args"].get("GameAction_NanoTemplate") for a in acts if a["type"] == "CastNano"), None)
            if nano:
                out[str(sid)]["channel_buff"] = nano
json.dump(out, open(os.path.join(ROOT, "data/projectiles.json"), "w"), indent=0, sort_keys=True)
print(len(out), "projectile spells")
