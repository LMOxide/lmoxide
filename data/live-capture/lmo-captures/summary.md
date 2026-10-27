# Live capture: LMO_Captures.cap

A Network Monitor capture of the live server, from the same 2016-09-29 session as
`LMOp1-lmo-exe.cap` (the parent folder), but captured almost completely. 10 of the 11 game-server
streams (37.18.193.16:7102-7119, zlib) are complete, and :7112 has one hole. That is 88,885
server RPCs and 84,382 PassBlobs. The player made 21 zone visits over 13 playfields: 4200, 4000,
1640, 1300, 4600, 1643, 1200, 8030, 8000, 8060, 4610, 6200 and 2210.

| File | Tool | Content |
|---|---|---|
| `capture-coverage.json` | `tools/re/live_coverage.py` | flows, decode state, analytics events |
| `playfield-objects.json` | `tools/re/live_playfield.py` | every UpdateMessage spawn (5,207) with position, playfield, time and, for characters, the full ProtoCoreCharacterIIR |
| `npc-movement.json`, `npc-movement-summary.md` | `tools/re/live_npc_movement.py` | MoveTask / LookAt / Stop / BCC per NPC, patrol routes |
| `dialogues.json` | `tools/re/live_dialogue.py` | the 15 Knubot dialogues (7 not in the demo) |

Server message times: `livecap.py` now inflates each zlib stream one TCP segment at a time. A
message gets the frame and timestamp of the segment that inflated its first byte.

## Into the server data

- `data/world_content.json`: `tools/re/extract_world_content.py` reads this capture
  (`live_records`). Playfields 1200, 1640, 1643, 2210, 4200, 6200, 8000, 8030 and 8060 now have
  `source: "live"`: they get the live server's own enemies, NPCs, smashables, buildies and
  triggers. Proposed spawns stay only where the player never came within 40 m. Summons,
  respawns and waves go to `live_dynamic` and are not spawned. The thresholds are in
  docs/TUNABLES.md ("Live spawns replace proposals"). Only these 9 playfields were replaced in
  the committed file. The extractor's output for other playfields differs from the committed
  file for reasons unrelated to this capture: possibly hand edits, not checked.
- `data/dialogues.json`: has the live dialogues (`source: "live"`), from
  `tools/re/extract_dialogues.py --live`.

## Enemy life by level (live)

Life = a base per level × a per-monster factor of 0.5 to 11.25. Base life by level:

| Level | 1 | 2 | 3 | 4 | 6 | 15 | 26 | 27 |
|---|---|---|---|---|---|---|---|---|
| Life | 62 | 90 | 114 | 132 | ~187 | 1314 | 1927 | ~2010 |

Examples at level 15: Mantis Nymph 657, Corrupted Chicken Suit Guy 1314, Forest Treeman 2956, and
a boss at 14782. Examples at level 26: Vespid 963, Rascal Compy 2890 and Boom Shroom 3854. The
levels are the ones the server gave this level-15 to level-26 player, so they may be scaled to the
player.

## Not decoded

The capture also holds the following, which are not decoded: AttackResultIIR (2,686),
EffectPackageIIR (9,986), MagicBuffIIR_c (13,937), AreaFormulaIIR_t (11,377), CoreProjectileIIR
(1,690), the quest IIRs and the loot IIRs.
