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
| Character level | `combat.rs` `LEVEL` | 30 | The level cap (`TWK_MaxPlayerLevel` tweak); the level itself is a choice. World content is level 1–3, so players one-shot it. |
| HealthPerLevel | `combat.rs`, `game.rs` `STAT_HEALTH_PER_LEVEL` | 100 | The recorded session's player had it. |
| PowerRatingAll | `combat.rs` | 50 | The recorded session's player default. |
| Minifigure level effect on power | `combat.rs` | none | The recorded session sent `PowerEffectPerLevel` 0 (minifigure level didn't change damage). |
| Enemy mitigation | `npc.rs` `ENEMY_MITIGATION` / `BOSS_MITIGATION` | 20% / 40% | Fitted to damage numbers in the recorded session (the client's `CalcMitigation` is a stub, so the real values were server-side). |
| Boss threshold | `npc.rs` `BOSS_HEALTH` | > 500 health | Assumed (the recorded boss had 1282). |
| Enemy damage | `npc.rs` `attack` | 1.5 × level + 1.5 | Fitted to the recorded session (3 at level 1, 4–5 at level 2, 5–6 for the level-3 boss). |
| Enemy attack interval | `npc.rs` `ATTACK_INTERVAL` | 2.5 s | Assumed. |
| Enemy aggro / leash / melee range | `npc.rs` `AGGRO_RANGE` / `LEASH_RANGE` / `MELEE_RANGE` | 8 m / 25 m / 1.8 m | Assumed. |
| AI tick | `npc.rs` `TICK` | 0.75 s | Assumed (movement updates for enemies). |
| Enemy respawn | `npc.rs` `RESPAWN` | 20 s | Assumed. |
| Death → despawn delay | `npc.rs` `hit` | 3 s for enemies; smashed debris stays until the respawn | Assumed. In the recorded session the debris was still there minutes later; when the real server removed or rebuilt it is unknown. |
| Proposed enemy level/health/spells | `data/world_content.json` (`source: "proposed"`), `extract_world_content.py` `THEMES`, `enemy` | world level from `THEMES` (4–19), dungeons their parent world's level + 1; health 60 + 15 × level | Only Tutorial, Pirate World and the pirate prison are from the recorded session; every other world's and dungeon's enemies are proposals. |
| Proposed enemy placement | `extract_world_content.py` `proposals` | camps of 3 (dungeons 2–3) on walkable navmesh cells (RDB 1000051) ≥ 26 m apart (dungeons 14 m), 1 per 800 m² of walkable area, max 40 (dungeons 1 per 250 m², 3–14); designers' combat areas (RDB 1000053 `ClearHate_*`, `Res_*`, ambush/spawn areas) first; ≥ 25 m from the arrival point, 12 m from resurrection points, 10 m from portals | Live spawn tables were server-side; density and spacing are guesses. |
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
| Charge level counter buff | not sent | — | Big Blast's tick also adds counter buff 31416 (no visual; the MagicBuffIIR_c counter update isn't decoded yet). The orb buff is re-cast every tick as live did. |
| Charge hold before auto-fire | `game.rs` `CHARGE_HOLD` | 3 s after full charge | Assumed. |
| Re-cast after an automatic fire | `game.rs` `CHARGE_RECAST_GAP` | re-sends refused until the client stops for 0.5 s | Assumed. A still-held button re-sends the cast after the server ends the channel. |
| Unreleased charge fallback | `game.rs` `CHARGE_TIMEOUT` | 6 s | Safety net if the release never arrives. |
| Dash length | `dash.rs` `start` | towards the clicked point, whole metres, at most the spell's Distance; full Distance along the facing without a point | Fitted to the 5 recorded For Rome! casts. |
| Dash timing | `dash.rs` `SPEED` / `SETTLE` | 37.5 m/s, end records 0.1 s after arrival | MovementChargeSpeed and timing from the recorded session. |
| Dash path hits | `dash.rs` `start` | enemies within the spell's Radius of the path, hit when the dash starts | Recorded: hits arrived 0–21 ms after the cast. |
| Dash spline value | `dash.rs` `path_record` | length / 35 | Empirical fit to the recorded casts; meaning unknown. |
| Samurai Warrior stance | not implemented | — | Its dash only comes with the Lotus Dash upgrade inside a channelled stance. |
| Summon recast block | `game.rs` `SUMMON_RECHARGE` | 2 s | The spell's real recharge wasn't enforced client-side; assumed. |
| Summon lifetime / looks / damage | `data/summons.json`, `npc.rs` `PET_DAMAGE` / `PET_FOLLOW` / `PET_SHOOT_RANGE` | 15 s, chosen models, 60 damage, 2 m, 8 m | The spawned monster templates are server-side; looks chosen by name. |
| Upgrade procs without client actions | not implemented | — | ~75 upgrades (Lionhearted, Meltdown, Block, Berserker…) only had server-side effects. |

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

## Jump pads

| Value | Where | Now | Source / reasoning |
|---|---|---|---|
| Trigger radius | `jumppad.rs` `TRIGGER_RADIUS` / `TRIGGER_HEIGHT` | 1 m horizontally, 2 m vertically | The original server launched within ~0.5 m of the pad (recorded session); widened for safety. |
| Click-to-move walk speed | `jumppad.rs` `WALK_SPEED` | 6 m/s | Assumed. Click-to-move only reports the path's end, so a pad there fires after this estimated walk. |
| Arc height | `jumppad.rs` `DEFAULT_APEX` | 4 m above the higher end | Recorded jumps used 4–6 m (`apex` in world_content.json where known). |
| Landing points | `data/world_content.json` `target` (`target_by`) | — | Exact only for the Tutorial (1300) and the pirate prison (4600) ("demo"); elsewhere paired by name or nearest landing marker. |
| Launch effect | `jumppad.rs` `Pad::effect` | mortar effect, fan effect for wind fans | Only mortar and fan pads were recorded; planes, catapults and lilies use the mortar's. |
| Pads and buildies | `jumppad.rs`, `buildie.rs`, `data/buildies.json` | the 5 recorded buildies (pirate tutorial 1300, Pirate World 4600) must be built before the pads under them work; every other pad works without building | Live, many pads were buildies, but the buildie placements were server-side and only the recorded ones are known (no client data marks a pad as a buildie: tutorial pads with the same mortar look needed no building). |
| Build tick and range | `buildie.rs` `TICK`, `BUILD_RANGE`, `PLAYER_RATE` | 62 progress per 0.1 s tick (a 1600 buildie takes 2.6 s); the builder must stay within 6 m | The rate is recorded; the tick is the item stat `BuildableServerInterval` (0.1 s, e.g. template 489061; also the 0.1 in the buildie's Destructible fields); the range is a guess. Built buildies stay built until the server restarts (live reset them at some point; when is unknown). The tutorial's scripted NPC builder (rate 19) isn't reproduced. |
| Levers, gates, mirrors, pressure plates | `scenery.rs` | spawned as scenery: CARS group 5 (not usable), gates `is_closed` false, no collision (kdtree 0); empty RDB bounds replaced by an 8×6×8 m box | Their behaviour (lever→gate links, plate puzzles) was server script data with no surviving trace (user's choice 2026-10-23: show them, block nothing, invent no logic). Whether a gate looks open depends on how the client picks its motion (`anim_1` = open state), which isn't known. |
| Re-trigger | `jumppad.rs` `State::on` | a pad fires again only after stepping off | Assumed. |

