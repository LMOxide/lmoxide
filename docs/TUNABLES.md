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
| Death → despawn delay | `npc.rs` `hit` | 3 s | Assumed (time for the death animation). |
| Proposed enemy level/health/spells | `data/world_content.json` (`source: "proposed"`) | levels 1–3, ~60–140 health | Only Tutorial, Pirate World and the pirate prison are from the recorded session; other worlds' enemies are proposals. |
| Hub test enemies and smashables | `data/spawns.json` | 3 enemies, 4 smashables | For testing; the real hub had no enemies. |
| Melee reach | `game.rs` `MELEE_REACH` / `AREA_RADIUS` | 2.5 m circle, 1.5 m ahead | Assumed. The client logs targeting "range 2.5, cone 180". |
| Default projectile range | `projectile.rs` `DEFAULT_RANGE` | 15 m | Used when a spell has no AttackRange (the Alien Avenger's blaster spell itself says 15 m). |
| Piercing projectiles | `projectile.rs` `PIERCING_BELOW_SPEED` | slower than 5 m/s pierce | Heuristic: the combine harvester ploughs through, shots stop at the first hit. |
| Damage variance | `combat.rs` `VARIANCE` | ±10% | The tweak `AttackUtils_DamageFlagVariance` (client data); listed because the distribution is assumed uniform. |

## Abilities

| Value | Where | Now | Source / reasoning |
|---|---|---|---|
| Charge hold before auto-fire | `game.rs` `CHARGE_HOLD` | 3 s after full charge | Assumed. |
| Unreleased charge fallback | `game.rs` `CHARGE_TIMEOUT` | 6 s | Safety net if the release never arrives. |
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
| Stars dropped | `npc.rs` `ENEMY_STARS` / `SMASHABLE_STARS` | 10 / 3 | Assumed. |
| Health orb drop chance | `npc.rs` `hit` | 35% per enemy | Assumed. |
| Loot bag pull | `loot.rs` `PULL_RADIUS` | bags of objects killed from farther than 4 m drop 4 m from the killer | Assumed. Live dropped bags where the object died (recorded session), and the client only collects within 7 m (`AutolootDistance`, client data) of the bag and the item, which lands 2 m from the bag. |
| Loot bag lifetime | `loot.rs` `BAG_LIFETIME` | 60 s | Assumed. |

## World and character

| Value | Where | Now | Source / reasoning |
|---|---|---|---|
| Portal trigger radius | `teleport.rs` `PORTAL_RADIUS` | 3 m | Portal positions are client data; the radius is assumed. |
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
| Pads always active | `jumppad.rs` | every pad works, no building needed | Chosen. Live, many pads were buildies that had to be built first. |
| Re-trigger | `jumppad.rs` `State::on` | a pad fires again only after stepping off | Assumed. |

