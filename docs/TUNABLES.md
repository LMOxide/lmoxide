# Assumed values

Values the server uses that are **not** in the client data, so they are guesses or fits. Compare
them with the live game when possible. Each entry gives the place in the code and where the
number came from. "Recorded session" means the original server's traffic captured in
`Data/default_windowsx86.demo` (client folder).

Values taken from client data (spell/item stats, tweaks RDB 1000516, MinifigureUpgradeData,
playfield resources) aren't listed; neither are protocol constants.

## Combat

| Value | Where | Now | Source / reasoning |
|---|---|---|---|
| Character level | `combat.rs` `LEVEL`, `progression.rs` | 30 without `--progression`; with it, from XP (below) | The level cap (`TWK_MaxPlayerLevel` tweak); the fixed level is a choice. World content is level 1–3, so level-30 players one-shot it. |
| HealthPerLevel | `combat.rs`, `game.rs` `STAT_HEALTH_PER_LEVEL` | 100 | The recorded session's player had it. |
| PowerRatingAll | `combat.rs` | 50 | The recorded session's player default. |
| Minifigure level effect on power | `combat.rs` | none | The recorded session sent `PowerEffectPerLevel` 0 (minifigure level didn't change damage). |
| Enemy mitigation | `npc.rs` `ENEMY_MITIGATION` / `BOSS_MITIGATION` | 20% / 40% | Fitted to damage numbers in the recorded session (the client's `CalcMitigation` is a stub, so the real values were server-side). |
| Boss threshold | `npc.rs` `BOSS_HEALTH` | > 500 health | Assumed (the recorded boss had 1282). |
| Enemy damage | `npc.rs` `enemy_damage` | (1.5 × level + 1.5) × the spell's WeaponHit AbilityMod | The base is fitted to the recorded session (3 at level 1, 4–5 at level 2, 5–6 for the level-3 boss; the recorded Scallywag shots did 3); AbilityMod is client data. |
| Enemy attack interval | `npc.rs` `ATTACK_INTERVAL` | 2.5 s between two casts of one enemy | Assumed. Each spell also waits its own cooldown: the longer of RechargeDelay and its own SetSpellRecastTime (client data, e.g. Scallywag shot 5 s, Dreadleg spin 20 s). SetSpellLineRecastTime (shared line cooldowns) isn't modelled. |
| Which monster casts which spells | `data/npc_spells.json` (`tools/re/extract_npc_spells.py` `RECORDED`, `FAMILIES`) | recorded: Scallywags (skeleton_pirate_1 XX_Shot 197370 + XX_Slash 197365, skeleton_pirate_2 XX_Shot 197338), Musketeer (XX_Shot 191502), Captain Dreadleg (slash 232292, spin 238132); 27 more monsters get the "XX_<monster> … castable" / "XX_boss <name> …" spells whose name names them (e.g. alien brute/ranger/reaper, merc brawler/blaster, corsair spin, islander scout trap, satyr shaman fire, Medusa, Hades, Kraken, Captain Blackheart = Dreadleg's spells incl. shot and bomb); the rest (trolls, gremlins, harpies, hounds, ...) keep the spawn's spell (XX_Axe Chop) | NPC templates were server-side: no client resource lists a monster's spells. The name matches are guesses; generic spells ("XX_Smack", "XX_Arrow Shot", "XX_Banana", ...) clearly belonged to monsters but can't be tied to one from the data. Spawn controls, summons, taunts, on-spawn/combat/victory triggers and auras are left out. |
| Enemy spell choice | `npc.rs` `engage` | among its spells that are off cooldown and reach the player, the highest AIPowerrating (ties random) | The live AI was server-side; AIPowerrating/AISpellCategory are client data, how the AI used them is a guess. |
| Enemy spell reach / keeping distance | `npc.rs` `reach_of`, `keep_distance` | a spell reaches AttackRange, at least 2.8 m; enemies with a melee spell close in to 1.8 m, ranged-only ones stop at 80% of their shortest range (at most 7 m, inside the aggro range) | Assumed. |
| Enemy cast timing | `npc_spells.rs` `cast`, `npc.rs` `engage` | cast records (BCC 16, 4, 10) at once, the spell's actions after its ItemDelay; the enemy stands still until then (and for channels that aren't self-centred, the whole channel); channels end with BCC action 7 | The recorded server sent action 4, then 10 with the hit; when exactly is unknown. Ending with 7 is recorded (Dreadleg spin). |
| Self-centred enemy spells | `npc_spells.rs` `self_centred` | TargetingMode 8, or a channel without TargetingMode | The recorded Dreadleg spin (no TargetingMode) was cast on himself. |
| Buffs without TimeExist on/from enemies | `npc_spells.rs` `DEFAULT_BUFF` | 3 s | Recorded for Dreadleg's "spin tired" stun (300 cs). A stun buff on the enemy keeps it standing. |
| Enemy projectiles | `npc_spells.rs` `projectile` | straight shots at a player home in and always hit when they arrive (flight time = distance / speed); ballistic or ground-targeted shots act where they land; shots along the facing or turned by RotY (fans, volleys) hit the first player within 1 m of their line | The recorded server reported each Scallywag shot as a hit (`CoreProjectileIIR` HitTarget); dodging rules unknown. |
| Enemy traps | `npc_spells.rs` `trap` | a PlaceTrap springs once when a player comes within 2 m, during its TimedLength | Assumed (the trap's own area formula radius is 2 m). |
| Enemy spell actions not run | `npc_spells.rs` `act` | SpawnMonster, server scripts, damage redirects, diminishing returns, buff counters | Some boss spells (Dreadleg bomb, Kraken ink bubble, Hades charge/groundfire, saucer ball, queen purge) therefore only show their cast. |
| Enemy aggro / leash / melee range | `npc.rs` `AGGRO_RANGE` / `LEASH_RANGE` / `MELEE_RANGE` | 8 m / 25 m / 1.8 m | Assumed. |
| AI tick | `npc.rs` `TICK` | 0.25 s | Assumed (how often enemies and pets decide; paths go out only when the goal moved, see below). |
| NPC movement speeds | `npc.rs` `ENEMY_SPEEDS` / `SLOW_ENEMY_SPEEDS` / `RUN_SPEED` | enemies 1396 2.1, 1397 4.2, 1398 8.4, 1399 2.1, 1400 8.4, 1401/1402 50 (Scallywags and Tavern Pirates 1.4/1.4/2.8/1.4/2.8); pets and story NPCs the players' (5.6) | From the recorded session (Pirates, Musketeers, Captain Dreadleg; Scallywags, Tavern Pirates). Other enemies' values were server-side NPC template data, so they get the Pirates'. Clients move characters along paths at 1397 (measured on the recorded NPCs); the server follows them at that speed. |
| Look refresh delay | `game.rs` `COSTUME_REFRESH_DELAY` | 1.5 s after entering a playfield | Assumed; long enough for the spawn records' first build. Works around player characters not animating until their look changes (not something the original server is known to have done). |
| NPC re-path distance | `npc.rs` `REPATH` / `ARRIVED` | new path when the goal is 1.5 m from the current path's end; stop within 0.3 m | Assumed. The original server sent paths only when its plan changed (and a StopIIR_c when an NPC stopped). |
| Enemy respawn | `npc.rs` `RESPAWN` | 20 s | Assumed. |
| Death → despawn delay | `npc.rs` `hit` | 3 s for enemies; smashed debris stays until the respawn | Assumed. In the recorded session the debris was still there minutes later; when the real server removed or rebuilt it is unknown. |
| Proposed enemy level/health/spells | `data/world_content.json` (`source: "proposed"`), `extract_world_content.py` `THEMES`, `enemy` | world level from `THEMES` (4–19), dungeons their parent world's level + 1; health 60 + 15 × level | Only Tutorial, Pirate World and the pirate prison are from the recorded session; every other world's and dungeon's enemies are proposals. |
| Proposed enemy placement | `extract_world_content.py` `proposals` | camps of 3 (dungeons 2–3) on walkable navmesh cells (RDB 1000051; not on navmesh islands under 600 m² and 5% of the walkable area, i.e. ledges and roofs) ≥ 26 m apart (dungeons 14 m), 1 per 800 m² of walkable area, max 40 (dungeons 1 per 250 m², 3–14); designers' combat areas (RDB 1000053 `ClearHate_*`, `Res_*`, ambush/spawn areas) first; ≥ 25 m from the arrival point, 12 m from resurrection points, 10 m from portals | Live spawn tables were server-side; density and spacing are guesses. |
| Proposed bosses and champions | `extract_world_content.py` `BOSSES` | boss per dungeon at its encounter area (else the walkable spot farthest from the entrance), level + 2, 10 × health; a champion (level + 1, 4 × health) at each `BA_SpawnChampion` area | Which monster is each dungeon's boss is inferred from area names (`BossEncounter_Start_Medusa`, `Res_hades`, …) and monsterdata names. |
| Proposed smashables | `extract_world_content.py` `proposals` | clusters of 3 themed crates/barrels, 1 per 900 m² (max 30; dungeons 2–8), away from camps | Guessed. |
| Story NPCs at cinematic points | `extract_world_content.py` `NPC_ACTORS` | level 1, 100 health, CARS group 5 | Like the recorded Video Game Guy (level 1–3, 93–171 health). |
| Hub test enemies and smashables | `data/spawns.json` | 3 enemies, 4 smashables | For testing; the real hub had no enemies. |
| Melee reach | `game.rs` `MELEE_REACH` / `AREA_RADIUS` | 2.5 m circle, 1.5 m ahead | Assumed. The client logs targeting "range 2.5, cone 180". |
| Default projectile range | `projectile.rs` `DEFAULT_RANGE` | 15 m | Used when a spell has no AttackRange (the Alien Avenger's blaster spell itself says 15 m). |
| Piercing projectiles | `projectile.rs` `PIERCING_BELOW_SPEED` | slower than 5 m/s pierce | Heuristic: the combine harvester ploughs through, shots stop at the first hit. |
| Damage variance | `combat.rs` `VARIANCE` | ±10% | The tweak `AttackUtils_DamageFlagVariance` (client data); listed because the distribution is assumed uniform. |

## Abilities

| Value | Where | Now | Source / reasoning |
|---|---|---|---|
| Charge level counter buff | `game.rs` `run_charge`, `projectiles.json` `counter_buff` | every channel tick after the press raises the spell's counter buff (Alien Avenger 31416, Tennis Ace 636386 with its own effect, Baseball Fielder, Fortune Teller, Lizard Man): the first tick adds it with count 1 (`MagicBuffIIR_c` mode 0), later ones update the count (mode 2, counter type 0); it lasts until the charge window ends and is removed with the channel buff | The action lists are the spell's (Use: CastNano; tick: CastNano + AddBuffCounter; end: RemoveBuff ×2); not in the recorded session, so the counter type (0) and the time left sent are assumptions. |
| Charge hold before auto-fire | `game.rs` `CHARGE_HOLD` | 3 s after full charge | Assumed. |
| Re-cast after an automatic fire | `game.rs` `CHARGE_RECAST_GAP` | re-sends refused until the client stops for 0.5 s | Assumed. A still-held button re-sends the cast after the server ends the channel. |
| Unreleased charge fallback | `game.rs` `CHARGE_TIMEOUT` | 6 s | Safety net if the release never arrives. |
| Dash length | `dash.rs` `start` | towards the clicked point, whole metres, at most the spell's Distance; full Distance along the facing without a point | Fitted to the 5 recorded For Rome! casts. |
| Dash timing | `dash.rs` `SPEED` / `SETTLE` | 37.5 m/s, end records 0.1 s after arrival | MovementChargeSpeed and timing from the recorded session. |
| Dash path hits | `dash.rs` `start` | enemies within the spell's Radius of the path, hit when the dash starts | Recorded: hits arrived 0–21 ms after the cast. |
| Dash spline value | `dash.rs` `path_record` | length / 35 | Empirical fit to the recorded casts; meaning unknown. |
| Channelled abilities | `actions.rs`, `data/spell_actions.json` | the spell's lists run on the server: PreUse/OnUse/channel start at the press, OnChanneling every ChannelingInterval for ChannelingTicks ticks, OnRechargeDelay after RechargeDelay, OnChannelingEnd at the release or after the last tick (then the server ends the client's channel with BCC action 20) | Lists and times are client data; that the channel ends by itself after the last tick (instead of holding on) is assumed. The meaning of lists 1/35/39 comes from the names of Funcom's "DEV TECH Channeling … Echo" test nanos. |
| Samurai Warrior stance | `actions.rs` `ENDS_WHEN_HIT` | being hit ends the stance (area attack); with Lotus Dash it dashes 0.5 s after the press (OnRechargeDelay) and at the end | The hit trigger was a server Lua script (LuaScriptID 3825); taken from the spell's description. The dash uses the usual dash speed; the ColumnDash's FloatValue1 (0.5) is unused. |
| Buffs without TimeExist | `actions.rs` `cast_nano` | last until the channel's full length is over (+1 s) unless the end list removes them first | Assumed. A buff's MitigationOverrideFlat (stances: +50%) reduces enemy damage while it lasts (`npc.rs`, total capped at 90%). |
| Ground zones | `actions.rs` `GROUND_DEFAULT_LIFETIME` / `GROUND_PULSE` | CorePersistentGroundEffect: lifetime Arg1 tenths of a second (else 3 s), its nano re-applied every 1 s after DelayTime; PlaceTrap: every Interval for TimedLength s | Arg1's unit and the pulse rate are guesses (the action is server-only, so the client has no code for it). |
| Shapes of area actions | `actions.rs` `act` | CoreFilteredAreaFormula: a sector of Angle degrees from AngleStart (else centred) around the facing, between Min and Radius; ColumnFormula: Distance long, FloatValue1 (else 1 m) to each side; CoreRandomPointAreaFormula: Max1 random points within FloatRadius | Field meanings inferred from the values. Crowd control (snares, knockbacks, stuns) isn't simulated, only shown as buffs. |
| Summon recast block | `game.rs` `SUMMON_RECHARGE` | 2 s | The spell's real recharge wasn't enforced client-side; assumed. |
| Summon lifetime / looks / damage | `data/summons.json`, `npc.rs` `PET_DAMAGE` / `PET_FOLLOW` / `PET_SHOOT_RANGE` | 15 s, chosen models, 60 damage, 2 m, 8 m | The spawned monster templates are server-side; looks chosen by name. |
| Upgrade procs | `procs.rs` | Lionhearted +10% damage 3 s after a special ("warriordiscipline" nano 429771, taken as its buff), Block +10% mitigation 3 s, Smash and Dash / Hunter / Skirmisher / Pickup speed +50 for 2 s, Berserker every 5th attack +100%, Brute Force +33% next attack after a kill, Boost Shield +33% defense against the next hit after a kill, Harden Up every 3rd normal attack +10% defense 3 s, Left Hook every 3rd normal attack +10% damage 3 s, Powered Barrier every 5th attack +100% defense next hit, Slippery Power/Defense +20% when not hit for 5 s, Meltdown (special kill: 0.05 AbilityMod pulses, radius 3, around the player for 5 s), Personal Space (special kill: 0.05 pulses every 0.25 s for 3 s where the enemy fell), True Grit (below 20%: 90% mitigation 5 s, once per 5 min), Escape Artist (below 20%: speed 3 s, once per 5 min), Second Wind (below 20%: heal 50%, once per 10 min), Reconstruct (heal 5% per kill), Sponge (heal 1% per hit), Regenerate (heal 2% when hit while channelling), Shields Up (+2% defense per hit while channelling, up to 10%) | Values and durations from the "XX_passive generic …" nanos (RDB 1000621) where they exist; heal amounts, the durations of buffs without TimeExist and Lionhearted's nano are assumptions. "Every n-th attack" counts casts. Defense bonuses divide damage by (1 + bonus). Charge Up (normal attacks add charges, up to 5 for 5 s; the special releases them around the player, radius 3, AbilityMod 0.5–1.5) and Knockback (every 3rd normal attack pushes surviving enemies 4 m) follow their nanos too; the knockback moves the enemy instantly along the line from the player (no navmesh check). Resourceful: +15 points of health orb chance; Professional: 25% chance of a health orb when the special smashes an enemy (both assumed). Not done: pulls and the specials' own knockbacks, cooldown changes (Specialist, Technician), Elemental Affinity. |

## Health and items

| Value | Where | Now | Source / reasoning |
|---|---|---|---|
| Regeneration | `npc.rs` `REGEN_DELAY` / `REGEN_SHARE` / `REGEN_TICK` | 5% of max per second after 5 s without damage | The recorded session had `HealthRegenOOC` 0.05 (unit assumed per second); the delay is assumed. |
| Potion heal | `game.rs` `POTION_HEAL_SHARE` | 50% of max | The heal amount was a server script. |
| Health orb heal | `game.rs` `HEALTH_ORB_HEAL_SHARE` | 25% of max | Assumed. |
| Potions | `game.rs` `START_POTIONS` | 5 per session, restocked at login | Assumed. |
| Stars dropped | `loot.rs` `roll` | enemies 3, smashables 1–2 | From the recorded session (every regular enemy bag held 3 stars, smashables 1–2). |
| Other drops | `loot.rs` `roll` | enemies: health orb 35%, part 6%; smashables: part 4%; champions (≥ 4 × regular health): 6 stars, orb, part, minifigure bag 15%; bosses (≥ 8 ×): 10 stars, orb, 2 parts, minifigure bag 40%, a chest; chest: 5 + 25 stars, 2 parts, minifigure bag 25% | The drop tables (LootTable ids) were server-side; proposals. Parts and bags are any minifigure's (which world dropped which is only a text id). |
| Team loot | `loot.rs` `SHARE_RANGE` | own bag for each teammate in play within 60 m, rolled separately | Only a bag's owner sees its items (client), so live must have given each member a bag; range assumed. |
| Duplicate minifigure bag | `game.rs` `DUPLICATE_BAG_STARS` | 50 stars | "Duplicates will be rewarded with stars"; amount unknown. Every minifigure is unlocked here, so every bag is a duplicate. |
| Parts → stars | `game.rs` `STARS_PER_PART` | 5 stars per spare part | ConvertMinifigureParts rate is not in the client. |
| Member chests | `loot.rs` `Tables.chests` | never dropped | Only members could open them (server check); only the free Super Chests drop. |
| Health orb drop chance | `npc.rs` `hit` | 35% per enemy | Assumed. |
| Loot bag lifetime | `loot.rs` `BAG_LIFETIME` | 60 s | Assumed. |

## World and character

| Value | Where | Now | Source / reasoning |
|---|---|---|---|
| Portal trigger radius | `teleport.rs` `PORTAL_RADIUS` | 3 m | Portal positions are client data; the radius is assumed. |
| Portals after arriving | `teleport.rs` `REARM_DISTANCE`, `game.rs` | after login or any teleport no portal opens until the player has been more than 8 m from every portal | The user's wish: arriving next to a portal (return points are 5–8 m from it) mustn't open it on the first step. |
| Portal → teleport window | `data/destinations.json` portals `window` (`tools/re/extract_portal_windows.py`) | the window whose arrival point names the source, else the destination's plain window | Which portal opened which window was server script data. Every portal opens its window (live behaviour inferred from the client: only server scripts open it). |
| Arrival when coming back through a teleport window | `data/return_points.json` (`tools/re/extract_return_points.py`), `teleport.rs` `return_point` | a walkable spot 5–8 m from the portal leading back to where the player came from, on the side facing the playfield's main arrival point, facing away from the portal (yaw convention assumed: atan2(dx, dz)) | Live arrived at source-specific `TP_From<source>` points (named in the teleport windows); their positions were server-side and are in no client data. World Map travel keeps its chosen spot; 6 portals without a nearby walkable spot use the main arrival point. |
| A world's portal to itself leads to the hub | `teleport.rs` `portal_at` | — | Inference (the Medieval Kingdom's only nearby portal is listed as leading to itself). |
| Collision radius stat | `game.rs` `STAT_CHAR_RADIUS` | 50 (0.5 m) | Assumed; needed for keyboard movement. |
| Command queue size | `game.rs` `STAT_COMMAND_QUEUE_SIZE` | 3 | Assumed. |
| Start position / team | `game.rs` `START_POS` / `START_TEAM` | hub plaza; Astronaut, Ballerina, Farmer | Chosen. |
| Collection | `collection.rs` | all 133 minifigures, level 10, every upgrade purchased | Chosen (requested). |
| Default upgrade selection | `collection.rs` `DEFAULT_ACTIVE` | one of each choice pair | Chosen. |
| Save interval | `game.rs` `SAVE_INTERVAL` | 10 s while moving | Implementation choice. |
| Teleport spots | `teleport.rs` `spots` | every resurrection point shown and unlocked | Chosen (requested). Live, players discovered them. |

## Progression (`--progression`)

| Value | Where | Now | Source / reasoning |
|---|---|---|---|
| Default | `main.rs` `--progression` | off: level 30, every minifigure at rank 8 | The unlocked collection was chosen (requested); progression is opt-in. |
| XP curve | `progression.rs` `next_xp` | NextXP = 50 × L × (L + 3): 200 for level 2 (recorded), each level 100 × (L + 1) more | Only level 1's NextXP (200) is recorded; the curve is assumed. |
| XP per defeat | `progression.rs` `kill_xp` | 7 × level, champions 14 × (level + 1), bosses 15 × level; smashables none | Recorded: level-2 Musketeer 14, level-3 boss Captain Dreadleg 45, tutorial level-1 Scallywags 28 (the tutorial isn't reproduced); no XP for smashables in the recording. |
| Level caps | `progression.rs` `level_cap` | Pirate World (4xxx) 11, Medieval (5xxx) 15, Space (3xxx) 19, Mythology (2xxx) 21, Dino (6xxx) 30, elsewhere none | Pirate World's 11 and the tutorial's none are recorded (PlayfieldFIIR_t); the others assume two levels above where the next world's proposed enemies start. XP keeps counting at the cap; the level follows on the next gain in a higher area. |
| Minifigure ranks | `progression.rs`, `data/progression.json` | rank 0..8, start at 0; a rank costs the stars up to the next MinifigureXPTables threshold (50, 60, 140, ...) and needs player level 3, 6, ... 24 (ranks 0..7) | Tables and level requirements are client data (RDB 1000525, lore tags 466-474); that a rank is bought with stars (the "minifigure XP" token 100) through LevelUpMinifigure follows the Collection GUI. |
| Upgrade tiers | `progression.rs` `unlocked_upgrades` | rank r unlocks tiers 1..r | The Collection's tier panel (one tier per rank purchase). |

## Combat level

| Value | Where | Now | Source / reasoning |
|---|---|---|---|
| Level a character fights at | `CharacterExt::combat_level`, `progression::combat_cap` | its level, at most the area's cap: the world caps (Pirate 11, Medieval 15, Space 19, Mythology 21, Dino 30), a side dungeon the smallest of those at or above its enemies' level, the hub none | The recording's level-1 hits (52-60 on 139-health Scallywags) match our damage formula at matching levels; without a cap level-30 characters one-shot level-5 enemies. Live showed "You have reached the maximum level for this area". |
| Item health stats | `combat::item_life` | MinifigureLife = max health - level x HealthPerLevel; MinifigureHealth = max health | The client adds level x HealthPerLevel to MinifigureLife (CalculateLife); sending the full value showed the bar half full. |
| Enemy targets | `npc::is_player` | player characters only | Scenery, jump pads, doors and collectibles are world members too; enemies were chasing them. |

## Upgrade shop

| Value | Where | Now | Source / reasoning |
|---|---|---|---|
| Upgrade prices | `shop::price` | a tier costs a minifigure rank step's stars (MinifigureXPTables): 50, 60, 140, 150, 250, 350, 400, 700 | Prices were server shop data; the XP table is the client's own star scale for ranks. |
| Minifigure level | `game.rs` purchase | one per upgrade bought (max 8) | Shown as the Collection's rank dots. |
| Without `--progression` | `CharacterExt::purchased` | everything bought; purchases refused | Matches the unlocked collection. |
| Older saves | `CharacterExt::purchased` | no purchases recorded: the tiers of the saved rank | Keeps upgrades bought with the earlier rank-up model. |

## Achievements

| Value | Where | Now | Source / reasoning |
|---|---|---|---|
| What earns an achievement | `data/achievements.json` `kind` (`extract_achievements.py`), `achievements.rs` | counters (kills, smashes, buildies, by name for the "Smash 50 Witches" kind), "Complete Any N", parents when all visible children are done, "Upgrade N" = minifigure level ≥ N, series = every minifigure of the series owned, "Defeat X" and the minifigure ones = boss named X (or the boss of dungeon X) defeated with that minifigure active, "Combat_Champion_X" = champion (or boss) named X defeated, "Combat_PA_Clear the X" = every resident enemy of dungeon X defeated in one visit, collectibles and "[World] Boombox" = that world object found | The criteria were server Lua; inferred from the names. Most exploration, seasonal events and Lost Creations have no rule (388 of 2144). No spawn has a champion's name yet, so the champion ones can't be earned until world events spawn them. |
| Exploration | `extract_achievements.py` `EXPLORE`, `encounter::explored` | 26 "Explore" achievements = walking into their `Achievement_*` / `BA_Explore_*` bounded area (matched by name; Dino "Lost Pirates"/"Beauty Salon" guessed); 13 have no area | The areas are the designers'; the binding was server script. |
| What counts as found | `discovery.rs` | collectible: used (ObjectInteraction; spawned with CARS group 1) or within 4 m; boombox: within 6 m of the "boombox 1" marker | Assumed; live's trigger was server script. |
| Journal | `achievements.rs` `on_event`, DB `character_defeats` / `character_visits` / `character_finds` | every defeat by name (count, first/last time), every playfield entered, every find | Server bookkeeping for the achievements and the API; the client shows none of it. |
| Who gets the credit | `npc.rs` `hit` | the player who dealt the killing blow | Assumed (group credit unknown). |
| Rewards | `achievements.rs` `grant` | AchievementRewardValue stars when the reward icon is the stars icon | Other rewards (the gold minifigure icons 550092/550093) have no data. |
| Granted at login | `achievements.rs` `login` | whatever already holds (with every minifigure owned at level 10: all "Upgrade 3/7" and the series ones), without pop-ups | Consequence of the unlocked collection. |

## Encounters (`encounter.rs`, data/encounters.json)

The original server ran these as Scry scripts on bounded areas; only the areas (RDB 1000053),
the arena doors and the tutorial's recording survive, so everything below is inferred.

| Value | Where | Now | Source / reasoning |
|---|---|---|---|
| Bouncer built under a player | `jumppad::activate` | launched at once | Avoids needing another step. |
| What engages an arena | `encounter::moved` | a player within 3 m of a `*Encounter_Start*` / `X_Start` area | Start areas are often thin strips across the entrance. |
| The arena's boss | `npc::boss_near` | the strongest proposed boss (8x health) within the area's radius + 60 m | No area-to-NPC binding in the data. |
| Doors | `encounter::door_hidden` | VisualOpen hidden and VisualClosed shown while engaged, exits hidden once won; visuals only (no collision) | Records as in the tutorial recording; shields (collision) aren't spawned. |
| Boss adds | `ADD_THRESHOLDS`, `ADDS` | 3 regular enemies of the area around the boss at 2/3 and 1/3 health | Live used SpawnMonster by health bands (e.g. the Kraken 59-60 %, 80-81 %); counts unknown. |
| Arena reset | `RESET_AFTER` | 20 s after no engaged player is near: boss healed, adds gone, doors open | |
| Ambushes | `AMBUSH_WAVE`, `AMBUSH_REST` | 2 waves of 3 (the area's nearest enemy kinds, around the player), then 5 min rest | The tutorial's recorded ambush (3 + 3). |
| Champions | `CHAMPION_HEALTH`, `CHAMPION_REST` | one per side dungeon ("Defeat the champion" area quests), named after an unbeaten champion of its world, 5x health, at BA_SpawnChampion or the farthest regular; a new one 10 min after | Champion names only exist as achievements. |
| Past a boss | `encounter::GATES` | ambushes, arenas and the champion spot farther from the playfield's arrival point than an arena's start stay quiet until that arena's boss is beaten (arenas without a boss don't gate) | Straight-line distance; inferred. |
| Pocket worlds | `encounter::pocket`, `npc.rs` respawn | every playfield but the open worlds (hub, 4200, 4210, 4600, 5200, 5210, 3200, 3210, 2210, 6200, tutorial): encounters run once and defeated enemies stay down while anyone is inside; when it empties, all of it resets (a new instance) | Live instanced dungeons per group. Open worlds: encounters come back after their rest, enemies after RESPAWN. |
| Arena walls | `encounter.rs` `SHIELDS`, `scenery.rs` | each "Boss…Shield" piece within 3 m of a closed (entrance or exit) door visual is a wall (collision 60624, as recorded) while that visual shows | Recorded in the tutorial: the shield spawned with KD-tree 60624 and lost it (VisualChange 4, 0) when the boss fell. Pairing by distance is inferred. |
| Not done | | boss buildies (Kraken 194851, Medusa mirrors, ...), boss phases/immunities, area quests, the Kraken's own script | |

## Tutorial

| Value | Where | Now | Source / reasoning |
|---|---|---|---|
| Trigger radius | `tutorial.rs` | 6 m; Video Game Guy's door 3.5 m | Live's trigger volumes aren't in the client data. |
| Escort | `tutorial.rs`, `npc::escort` | Video Game Guy follows the player and fights like a pet, one per player | Live: escort script 597462 ("Escortee=50000:1896"); its behaviour is server script. |
| Scripted Scallywags | `tutorial::script` | the recorded spawns, at least 75 health, gone after 15 min, per trigger | The recording caught some mid-fight (health 1). |
| Step order | `tutorial::Run::moved` | a trigger fires only after the one before it and once every enemy its steps spawned is beaten; the pads after the summoned pirate (195056) and the ambush (195055) open per player when those waves are beaten | Live hid them behind blockers until the waves fell (recording). A relog mid-way counts the steps west of the player as done. |
| Who plays it | `game.rs` | anyone in 1300 without tag 4121 | Live ran it for new characters. |
| Not done | | the MinimumHealthCap buff 236432, quest records 279/78, SpawnCompass, boss arena doors | |
| Pirate World cannon | `cannon.rs` | building the cannon (4600) fires at the fort wall 0.5 s later; the wall falls, 1 s later the Guard Captain cinematic runs and ends in the jail (4610) | Recorded (recs 29345-30059): shot item 234304 / spell 234303 from the muzzle at 20 m/s, effects 463510/463511, wall health 100000 so only the cannon breaks it; the delays are guesses (the recording's times aren't usable). |
| Wall and cannon reset | `cannon.rs` `WALL_RESPAWN`, `RESET_AFTER` | the wall comes back after 2 min, the cannon goes back to bricks 5 s later | Assumed: live gave each player the scene; a shared world needs them back for the next player. |

## Mirror puzzles (`mirrors.rs`, data/mirror_puzzles.json)

| Value | Where | Now | Source / reasoning |
|---|---|---|---|
| The puzzle | `tools/re/extract_mirror_puzzles.py` | each mirror's two markers face where the light leaves it (the one 45° from the mirror's facing in the open pose, 225° in the placed pose: the editor's "+"/"-" names don't follow the poses); the next mirror or target in line receives it | Client data (the markers); the user chose a working puzzle over scenery (2026-10-25). |
| Light source and start | `PUZZLES` roots | the light falls on mirror "1" (Armory) and on L and R (Gauntlet), where the rooms' own beams (level geometry) end; every mirror starts at "-" (unsolved) | The beams match; the start positions are a guess. |
| Turning | `mirrors::turn` | effect package 474576 (blend setup 474600 `anim` = 1: `open`, rest in `anim_1`) for "+", 474575 for "-"; its stone-scrape sound plays from the motion | The packages are client data; that live used them for the mirrors is inferred (the recording sent the same kind of package to jump pads). |
| The light | `mirrors::BEAMS` | `particle_generic_lightbeam_horizontal_<n>m` (n = length / 2, rounded, 1–7) on an invisible dynel at the "+"/"-" marker of every mirror the light falls on (VisualChange state 0, attractor 35) | The rooms' own first beams are these particles (RDB 1000058); live's XX_Invisible dynels carried particles that way (recording). That live drew the mirror-to-mirror light like this is inferred. |
| Rewards | `opens` | Armory: the barrier beside the pedestal (the Thunderbolt arena's exit) opens; Gauntlet: the gate hides | The pedestal ("complete" marker) stands at that barrier; the gate has two light markers. Inferred. |
| Reset | `RESET_CHECK` | back to the start when nobody has been in the playfield for up to 30 s | Live gave each group its own instance. |

## Gates (`gates.rs`, data/light_gates.json)

| Value | Where | Now | Source / reasoning |
|---|---|---|---|
| Medusa's light gates | `tools/re/extract_light_gates.py` | a gate with a "door light" marker in front has a mirror buildie (item 489061, look 489796) at each "mirror light" marker facing it (1.8 m below the marker); built, it beams light (mirrors.rs beam particle) and with every mirror of the gate built the gate plays its open motion and loses its collision | The user's memory of live (2026-10-25); marker pairing is client data; build values copied from the recorded buildies; no collision once built. |
| Gate collision | `GATE_COLLISION`, `WALLS` | while shut: KD-tree 2260 (the static gate model's CollisionMesh2) on the gate, and the arena shields' 6 m wall (60624) on the gate's invisible "door collision" piece | The gate mesh alone left gaps to squeeze around (user, 2026-10-25); the door collision pieces are client data, their live mesh unknown. |
| Gauntlet piston room | `pistons.rs` | each of the 15 pistons pushes out (open motion 474576) for 1.5 s every 4 s, staggered 0.7 s along its row; anyone 0.5–4 m in front of it is thrown (jump-pad arc, 1 m apex, no damage) onto the first lower floor straight ahead (the walkway a metre below its ledge, as the user saw live), tumbling (BCT substate 24 "Knockbacked" and effect 623093 `launched`: the minifigure graph's `launch` motion; from client disassembly, untested by eye); only while someone is in the Gauntlet | The pistons' server script ("JumpToPoint=AI_JumpPiston…;Height=1") is client data, its points weren't shipped; timing and landings are guesses from the navmesh; no damage per the user (the nano has a 15-damage WeaponHit). |
| Gauntlet Tiki room | `tiki.rs` | the skull ("Tiki Room Skull Forcefield Holder") wears the red-orange forcefield bubble (particle 412053) and, while anyone is in the room, every 4 s fires a pattern after a 1.2 s ground-fire warning: 3 fireballs out of one side (N, E, S, W by turns), 1 out of every side, or a 16-shot spiral; fireball item 519852, 250 damage, 10 m/s, 12 m (the spiral 18 m); no warning before the spiral; four statue buildies (look 152320, item 446283's build values) at the "statue N particle" markers glow (532195) once built; all four drop the forcefield, stop the fire and open the east gate | Pieces, items and particles are client data; the patterns are the user's memory (lines, four lines, spiral), the timing guessed. |
| Gauntlet plate room | `plates.rs` | stepping on a plate (2.5 m tile) swaps its red look (`_b_`) for the green (`_c_`); all 30 green open the east gate; click-to-move lights the plates along the path; reset once the Gauntlet is empty | The user's memory of live; plates and areas are client data. |
| Arena exit gates | `EXIT_GATES` | a LEGO gate within 3 m of an arena's exit visual (Medusa's) opens and shuts with that exit | Inferred from the placement. |
| Reset | — | built mirrors and opened gates stay so until the server restarts | As the other buildies. |

## Jump pads

| Value | Where | Now | Source / reasoning |
|---|---|---|---|
| Trigger radius | `jumppad.rs` `TRIGGER_RADIUS` / `TRIGGER_HEIGHT` | 1 m horizontally, 2 m vertically | The original server launched within ~0.5 m of the pad (recorded session); widened for safety. |
| Click-to-move walk speed | `jumppad.rs` `WALK_SPEED` | 6 m/s | Assumed. Click-to-move only reports the path's end, so a pad there fires after this estimated walk. |
| Arc height | `jumppad.rs` `DEFAULT_APEX` | 4 m above the higher end | Recorded jumps used 4–6 m (`apex` in world_content.json where known). |
| Landing points | `data/world_content.json` `target` (`target_by`), `tools/re/extract_world_content.py` `pair_jumps` | — | Exact only for the Tutorial (1300) and the pirate prison (4600) ("demo"); elsewhere guessed: named routes (6200's XToY1 → XToY2 → XToYEnd: "chain"), matching names, a pad standing on a landing point throws to the next one listed ("onward": 2210's clouds, lilies, 1690's climb), else the landing marker listed next to the pad or the nearest. Landing markers within 4 m of a pad are skipped for the last two: they are the way back's landing (5200's catapult pairs, which bounced players backwards). Invisible pads tagged JUMP with a script (2210's third cloud, 6200's SwampToPond2/3) are pads. |
| Chained jumps | `jumppad.rs` `Pad::throws_on`, `MAX_HOPS` | landing on an invisible pad, a lily pad or a named route's middle stop throws the player on at once (up to 8 hops); landing on any other pad waits for a step off | Assumed. The invisible cloud jumpers sit exactly on the clouds' landing points, and lilies float on water nobody can step onto, so live must have thrown on from them; visible pads someone lands on are often the way back (5200). |
| Pads that don't launch | `jumppad.rs` `Pad::launches` | only pads tagged JUMP launch; the mortars of Pirate Trials' cannonball course (4300, tag K28B) and an idle wind fan (28EI) are shown but inert | Their tags differ from every other pad's; the quest "Run safely through the Cannonball course" makes them hazards (not done). `tools/re/check_jumppads.py` checks the landing points against the navmesh. |
| Launch effect | `jumppad.rs` `Pad::effect` | mortar effect, fan effect for wind fans | Only mortar and fan pads were recorded; planes, catapults and lilies use the mortar's. |
| Pads and buildies | `jumppad.rs`, `buildie.rs`, `data/buildies.json` | the 5 recorded buildies (pirate tutorial 1300, Pirate World 4600) must be built before the pads under them work; every other pad works without building | Live, many pads were buildies, but the buildie placements were server-side and only the recorded ones are known (no client data marks a pad as a buildie: tutorial pads with the same mortar look needed no building). |
| Build tick and range | `buildie.rs` `TICK`, `BUILD_RANGE`, `PLAYER_RATE` | 62 progress per 0.1 s tick (a 1600 buildie takes 2.6 s); the builder must stay within 6 m | The rate is recorded; the tick is the item stat `BuildableServerInterval` (0.1 s, e.g. template 489061; also the 0.1 in the buildie's Destructible fields); the range is a guess. Built buildies stay built until the server restarts (live reset them at some point; when is unknown). The tutorial's scripted NPC builder (rate 19) isn't reproduced. |
| Levers, gates, pressure plates | `scenery.rs` | spawned as scenery: CARS group 5 (not usable), gates `is_closed` false, no collision (kdtree 0); empty RDB bounds replaced by an 8×6×8 m box | Their behaviour (lever→gate links, plate puzzles) was server script data with no surviving trace (user's choice 2026-10-23: show them, block nothing, invent no logic). Whether a gate looks open depends on how the client picks its motion (`anim_1` = open state), which isn't known. |
| Re-trigger | `jumppad.rs` `State::on` | a pad fires again only after stepping off | Assumed. |

