# Spawn research: which monster each SpawnMonster hash is

Hand-off for a research session (written 2026-10-29). The server can now spawn monsters from
spells. What's missing is the data: which NPC template each SpawnMonster action means. Read this
first, then `docs/TUNABLES.md` ("Spawned monsters", "Orbiting shots", "Encounters") and
`docs/CONTENT.md` (npc `spawn_hash`, server-made spell ids).

## How spawning works now

- A spell's `SpawnMonster` game action names the original server's monster template by a
  4-character `StringHash` (e.g. `FXCX`). Those templates were server-side and aren't in the
  client, so the hash means nothing until we match it by hand.
- Content: an NPC record (`content/npcs/*.json`) with `"spawn_hash": "FXCX"` is what that hash
  spawns. Its `lifetime` is how long it stays (none: 5 min, `npc.rs` `SPAWNED_LIFETIME`).
- Server (`npc.rs` `spawn_monster`, called from `actions.rs` for players' spells and
  `npc_spells.rs` for enemies' spells): it spawns the template where the action applies (ApplyOn 1:
  the caster, else the spell's target: a clicked point, an enemy's or a player's position). Then it
  casts the action's `NanoTemplate` on the spawned monster.
  - Kinds `enemy`/`boss`/`champion`: they fight the caster's player and drop nothing (live: 28
    summoned adds, no bag). Templates named for hashes become anchors otherwise.
  - Kinds `npc`/`trigger`/`summon`: invisible friendly anchors nobody fights; orbiting shots
    (ProjectileType 3, `projectile.rs` `launch_orbit`) circle them.
  - A boss arena's reset removes what its boss spawned (`npc::despawn_spawned_by`).
- An unmatched hash spawns nothing (debug log "no template with this spawn_hash").
- Boss arenas no longer get invented health-share waves (user, 2026-10-29). Adds come only from
  the boss's own SpawnMonster abilities. Other waves wait for a separate script built from
  footage or the client's scripts, which is not part of this research.

## What's matched

| Hash | Spawns | Evidence |
|---|---|---|
| `FXCX` | `npc/main-pierrot2` (TDB 51000 #1058, invisible look 51057), 6 s | Live capture, Dinosaur Rise 6200: a Sad Clown's casts. See "How FXCX was matched". |
| `P9TE` | `npc/merc-engineer-drones-holder` (TDB 51000 #1464, invisible look 51057), 11 s | Name match; the client's drone spells and item (see "The Space Merc Engineer's drones"). |

## Found 2026-10-29 (second session), not yet in content

Live capture: the caster's `BehaviorControlCenterIIR_c` names the spell id (u32 BE at about +44..+52),
and the spawned character's `SimpleCharFullUpdateIIR_t` follows within a few frames
(`research/lmoxide-notes/spawn-research/spawn_casts.py <live.jsonl>`). Lifetimes: first record
naming the instance to its `n3ToClientFadeIIR_t`.

| Hash | Spell | Caster (live) | Spawns | Lifetime | Evidence |
|---|---|---|---|---|---|
| `U1XT` | 663959 Queen's Guard (player) | player, 1200, 9 casts | TDB #3002 Queen's Guard = `npc/queen-s-guard` (352446, CARS 0, lvl 30, 750 HP) | **10 s** (fade 10.1 s in 8 of 9; one killed at 8.2 s) | live |
| `E9ER` | 338311 Call to Arms | Alpha Wolf #2887, 8030 | one #1259 Summoned Wolf = `npc/summoned-wolf` per cast | none seen (killed 2–11 s) | live |
| `68MJ` | 379451 "alien reaper egg spawn" | Alien Pod #2873, 8060 | one #1483 Mantis Nymph = `npc/mantis-nymph-3` per cast (not a reaper egg, despite the name) | none seen (killed) | live |
| `JVYN` | 236106 / 236107 XX_Spawn | Spider eggs #905 / #985, 4200 | #861 Spiderling = `npc/spiderling` (4 after one 236106 cast) | none seen | live |
| `SS34` | 661545 | Funhouse Poison Hazard Dummy #2961, 8030 | #2963 XX_Invisible Funhouse Poison Hazard Anchor (60 seen, CARS 5, lvl 15) | **16.1 s** (all 60) | client chain + live |
| `YY1Z` | 635595 / 636105 | (projectile chain below) | #2913 XX_Invisible Fire Geysir (9 seen in 8060) | **6.0 s** (all 9) | client chain + name + live presence |

- `SS34` chain: the dummy's buff 661554 = CastNano 660139 every 1 s, forever; 660139 =
  CoreRandomPointAreaFormula radius 18 m, one point → 661545 SpawnMonster SS34. Live: an anchor about
  every second within ~15 m of the dummy (the dummy itself lives 10.1 s). Each anchor shows buff
  661546 (Projectile, item 660600, falling from 5 m) and area 661548.
- `YY1Z` chain: 636101 = CastNano 636102 every 0.5 s; 636102 = random point 7–8 m (two
  formulas) → 636103 Projectile item 636104 "XX_medieval pf kingdom dragon geysir projectile"
  (EffectPackage 630429), whose hit casts 636105 = SpawnMonster YY1Z. The 636105 cast itself isn't in
  the capture. The geysir casts 659526 on itself: CastNano 636107 every 0.25 s = radius 2 m
  area, 636106 WeaponHit AbilityMod 0.5.
- Queen's Guard today summons a guessed `npc/royal-guard` for 15 s (`skills/queen-s-guard.json`
  `summon`); live says `npc/queen-s-guard` for 10 s.
- The capture's "Gargantuan" #2971 (8030) is an enemy (CARS 3), not the player's Gigantuan.
- Player hashes `XKRD`, `JXNM`, `HH3V`, `XVC4`, `QTVR`, `8W1V` are never cast in the capture (their
  ids only occur in a large spell-list blob).

Name matches (TDB 51000) to use as `source: guess`:
- `PRLO` → #2708 "XX_boss hades groundfire"
- `JIMV` → #2505 "XX_champion aura dread spheres invis anchor"
- `MCS2` → #2739 "laser drones invis anchor"
- `ZVJ2` → #2766 "firewall invis anchor"
- `47LU` or `1NZV` → #1455 "XX_space pd port boss queen purge npc" (which of the two is unknown)
- `R9TE` → #2847 "XX_Invisible Funhouse Hazard Anchor"
- `Q74I` → #2768 / #2769 "XX_Invisible Dragon Summoner (Anchor)"
- `9MCI` → #2765 "XX_Champion Minion Spawner"
- `N1GC` → #711 "Summoned Satyr"
- `XNCU` → #591 "Pillar of Lightning"
- `OPTG` → #2702 "XX_Electric surge lightning bolt"
- `W71R` → #2632 "lava monkey summoner" or #2635 "lava explosion"
- `RUQN` (Dragon's Breath): no name match

The hashes appear nowhere in the client but the spell records (1000621); no hash→template table
exists client-side, so matching stays evidence-based.

## The Space Merc Engineer's drones (done 2026-10-30)

The made-up bomb ring (374158, server-made 2000374158/2000374159, a grenade-look anchor) is
replaced by the client's data:

- **374162** "XX_merc engineer drones castable": SpawnMonster `P9TE` on his target, recast 7.5 s,
  AttackRange 15 m.
- **P9TE** = TDB 51000 **#1464 "XX_merc engineer drones holder"** (name match; invisible look
  51057 as other anchors): `npc/merc-engineer-drones-holder`, 11 s.
- **374164** "XX_merc engineer drones buff": four orbiting (ProjectileType 3) item **374203**, 110°/s,
  3 m out, RotY 0/180/270/90.
- **Item 374203** "XX_merc engineer drones projectile": EffectPackage 376220 (the drone particle and
  its boom), on hit 374204 = WeaponHit AbilityMod 2.5.
- Guesses: the castable's SpawnMonster has NanoTemplate 374164 added (the client names none, so who
  casts the buff on the holder is unknown); 374164 has TimeExist 1000 added (10 s, the drone
  particle's length and the user's memory; the server's orbit default is 5 s); whether a drone
  bursts on its first hit.

## The hashes still to match

All 119 SpawnMonster actions in the client (68 hashes, 82 spells) are in
`research/lmoxide-notes/spawn_actions.json` (`spawn_actions.py` regenerates it; it also lists
StartServerSideScryScript actions). By priority:

1. **Spells the content already casts** (they run today and spawn nothing):
   - `47LU` ×4 + `1NZV` (NanoTemplate 366112): Alien Queen's purge 366110;
   - `PRLO` (Arg3 620170, String0 "IdentifyHadesFire"): Hades' groundfire 623501;
   - `JIMV` (579610, 636113), `MCS2` (635567), `ZVJ2` (636582): the champions' Dread Spheres,
     Laser Drones and Firewall auras' anchors;
   - `RUQN`: Dragon's Breath 321685 (nobody casts it in content).
2. **Players' abilities still on guessed pets** (`summon` blocks in content/skills, run through
   `npc.rs` `summon`, not SpawnMonster). Each needs its hash matched before it switches over:
   - Anchor `XKRD` (NanoTemplate 236376 "main captain2 hate transfer");
   - Gigantuan `JXNM` (654691 "main s14 crazyscientist2 npc spawn wrapper", Arg3 654690);
   - Queen's Guard `U1XT` (667151, Arg3 654690): **matched**, see "Found 2026-10-29";
   - Shadow Puppet `HH3V` (508258 "main jester2 marker", Arg3 659020);
   - Smoke Bomb `XVC4` (595266);
   - Team Zombie `QTVR` ×3 (642804, Arg3 659775);
   - UFO `8W1V`.

   Switching one over means removing its `summon` block (as From Bad to Worse), adding its hash to
   `NO_LONGER_SUMMONS` in the `skills.rs` test, and checking what its NanoTemplate chain does.
3. **Boss and enemy kits not imported yet** (their spells aren't in content). Among them:
   - Crikey the Kraken `LS1V`/`MUC9`/`0GOL` (616742, by health bands: criteria 520/522 59-60 %,
     80-81 %);
   - Hades summon `1XMM` (485835);
   - Captain Pete's monkeys `QQVS`/`LR2L`;
   - Stormhammer's pillar/satyr `XNCU`/`N1GC`;
   - Alien Queen defense `YSFN`/`2L72`;
   - alien mindslicer `NQKR`/`ROT1`/`25XL`, overlord `J27F`, reaper egg `68MJ`, buzzer hive
     `VOOL`;
   - police HQ mech drones `P9TE`/`4WD1`, Dreadleg ship `E2II`, banshee `5UK2`, champion minions
     `9MCI`, dragon geysir `YY1Z`, tombstones `YMS9`/`9NM9`;
   - and more in the JSON (skip DEV TECH / dev ones).

   world-events.md (same folder) maps several of these to their arenas.

Also open: what the SpawnMonster args mean. Our guesses:
- `Value` (1; the Queen and Team Zombie repeat the action instead of raising it, so not a count);
- `Value2`;
- `Arg3` (a spell, e.g. 654690: maybe what the monster casts on spawn, or its AI kit);
- `String0`;
- `RelYPos`;
- `CollisionFlag` (snap to ground?);
- `HateValue`;
- criteria 520/522 (health %).

The server ignores all of these except StringHash and NanoTemplate.

## Sources, best first

1. **Client names (offline, fast).** TDB 51000 has 1612 NPC template names
   (`research/lmoxide-notes/spawn-research/npc_template_names.py <lmoxide/tools/re>`, run with
   `tools/re/py`). Spawned monsters are often named after the spell: "main pierrot2 hate transfer"
   → template "main pierrot2"; "merc engineer drones castable" → "XX_merc engineer drones
   holder"; "champion aura firewall spawn anchor" → #2766 "XX_champion aura firewall invis anchor";
   laser drones → #2739; "boss hades groundfire spawn" → #2708 "XX_boss hades groundfire"; dread
   spheres probably the same pattern. "main captain2", "main jester2" and the other players' spawns
   have no TDB name: try other text categories, the RDB NPC tables, or the live/beta captures.
   A name match is a guess: mark it `source: guess` (or `client` for the look) with a comment
   saying how it was matched.
2. **Live capture `LMO_Captures.cap`** (2016, 13 playfields: 4200, 4000, 1640, 1300, 4600, 1643,
   1200, 8030, 8000, 8060, 4610, 6200, 2210). A cast's MagicCommand, then an `UpdateMessage` op 0 +
   `SimpleCharFullUpdateIIR_t` for the spawned monster (name, monster data, template id 1058-style,
   CARS group, level, health), then its despawn or fade gives template, look and lifetime.
   - Tools: `tools/re/livecap.py` (`--jsonl` dump; one exists at
     `/tmp/claude-1000/-home-dgmastertemple-Dropbox-Development-projects-lego-minifigures-online/fd40152d-e907-4ff2-b11b-10b91516db46/scratchpad/live.jsonl`
     while that tmp survives) and `tools/re/live_playfield.py` (decoders; spawns are in
     `data/live-capture/lmo-captures/playfield-objects.json`).
   - The scripts used for FXCX are in `research/lmoxide-notes/spawn-research/`
     (`capture_records_naming.py`: every record naming given instance ids).
   - **Timestamps are microseconds** (`ts_raw / 1e6`). Older outputs' `t` is 10x too small.
   - The 28 summoned adds in the capture (loot research) are the obvious next targets: find which
     boss cast what just before each appeared.
3. **Beta capture `LMO_Packets.zip`** (2013-14). Keep its findings in its own doc, never merged with
   live (`research/beta-capture-findings-2026-10-28.md`).
4. **The user's footage and memory** (they check footage on request). Ask when a guess shapes
   gameplay: counts, positions, lifetimes, what it looks like.

## How FXCX was matched (the method)

1. `data/live-capture/lmo-captures/playfield-objects.json` had five "main pierrot2" characters in
   6200 (monster data 51057, template 1058, CARS 5, level 30, radius 0) a few seconds apart.
2. In the dump, the records around them showed CoreProjectileFullUpdateIIR with the anchor as target
   (the clouds, type 3, item 222206, spells 222171/222197/222198). That tied the anchor to From Bad
   to Worse (196059 → SpawnMonster FXCX → NanoTemplate 245949 → 245964 → the three orbs).
3. Lifetimes came from the same records: MagicBuffIIR_c of the orb spells on the anchor (500 cs),
   n3ToClientFadeIIR_t at 6 s, and the clouds' despawns at about 5.2 s.

## What to deliver

- For each hash matched: the NPC record (`spawn_hash`, kind, look, `lifetime`, `source`, a
  comment with the evidence). Use the existing template when there is one, and import any missing
  spells and items.
- A row per change in `docs/TUNABLES.md` ("Spawned monsters" lists the unmatched hashes: keep it
  current).
- `!test` scenarios for what can be tried (`data/test_scenarios.json`; the user tests: give them
  test cases).
- Check with `cargo run -p lmoxide-content -- check --warnings` and `fmt`, and run
  `cargo test -p lmoxide-server`. The `same_as_the_old_files` tests list deliberate differences
  from the archive.
- Commit content (its own repo in `content/`) and the server repo separately. Restart the LAN
  server with `tools/lan-server.sh --restart --release`.
