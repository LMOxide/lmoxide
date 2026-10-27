# NPC movement in the live capture

Made by `tools/re/live_npc_movement.py` from `../LMO_Captures.cap` (the livecap `--jsonl` dump) and
`playfield-objects.json` (NPC names, monster_data, spawn points, zone visits). The full data is in
`npc-movement.json`: 446 NPCs over 16 zone visits, each with its time-ordered MoveTask, Stop,
LookAt and BCC records and a summary, plus an aggregate per (name, monster_data).

## Formats verified

All layouts come from the native Linux client (symbol names, `tools/re/linux_fn.sh`). Each one was
checked by requiring every record to parse to the exact end of its PassBlob. The tool's docstring
has the field-by-field layouts. Fields whose meaning is not known are marked `?` / `*_unknown`.

| Record / task | Client function | What is known |
|---|---|---|
| MoveTaskIIR_c header | `MoveTaskIIR_c::ReadSubClass` | u8, vec3 start, then `MoveTaskFactory::Read` (u8 type + task) |
| task type ids | `MoveTaskFactory::Initialize` + static init 0x17781d9 | 0 Wait, 1 Path, 2 Rotate, 3 Jump, 4 List, 5 Ballistic, 6 Animated, 7 Formation, 8 Track, 9 Link, 10 SetPos, 11 GroundAlign, 12 ParameterizedBallistic, 13 KnockUp, 14 ActiveDodge, 15 Attack |
| Wait (base of Path/List/Link/Ballistic/KnockUp) | `WaitMoveTask::Read` | u8 flags: bit0 means forever (no floats follow), otherwise f32 duration + f32 remaining |
| Path | `PathMoveTask::Read` | u16 flags (speedup/facing enums, optional minbasespeed/maxspeed floats), points or u16 pair, SplinePath |
| SplinePath | `operator>>(BinaryStream&, SplinePath&)` | u8 loop, u32 point index, u8 ?, f32 ?, f32 ? |
| List | `ListMoveTask::Read` | Wait part, u8 idx (current child), u8 size, child tasks |
| Track | `TrackMoveTask::Read` | ID32 target, u8, f32, f32, u8 |
| Attack | `AttackMoveTask::Read` | SplinePath, points, u32, ID32 target, u32 (the client's click-to-move) |
| Ballistic / ParameterizedBallistic / KnockUp | `BallisticMoveTask::Read`, `ParameterizedBallisticMoveTask::Read`, `KnockUpMoveTask::Read` | from / to vectors ("from=", "to=" in the client log text), the rest ? |
| Rotate, SetPos, Formation, Link, Jump, GroundAlign, Animated, ActiveDodge | their `::Read` | sizes are known; only Rotate's quaternion and Formation's offset are named |
| CharDCMoveIIR_t | `PlayerMoveUpdateData::Read` | flag-dependent: pos, rot (2 or 4 floats), align mode, vel, extra vec3, f32, optional u16, 2 u8 |
| StopIIR_c | `StopIIR_c::ReadSubClass` | 7 f32 (pos + quaternion) + 2 u8 |
| LookAtIIR_t | `LookAtIIR_t::ReadSubClass` | ID32 target, u32 ?, u32 ? |
| BehaviorControlCenterIIR_c | `BehaviorControlCenterIIR_c::ReadSubClass` (switch 0x2f324b0) | u8 action; 11/12 carry a u32 substate; command actions carry a u32 CommandType_e, then `Command_c::ReadBaseClass` (u32 length + CommandMessage protobuf), then the class's ReadSubClass |
| BCC command subclasses | `MagicCommand_c`, `TransitionCommand_c`, `UseDynelCommand`, `ReviveCommand_c`, `CycleToNextWeaponCommand::ReadSubClass`; BCCAction/Limbo use the empty `Command_c::ReadSubClass` | the CommandType_e to class map comes from the `Command_c::RegisterCommand` calls; each class's vtable +0x100 entry was checked |

## Parse coverage (all server records, NPCs and players)

| Record | Total | Exact | Share |
|---|---|---|---|
| MoveTaskIIR_c | 6264 | 6264 | 100% |
| CharDCMoveIIR_t | 6752 | 6752 | 100% |
| LookAtIIR_t | 1211 | 1211 | 100% |
| StopIIR_c | 296 | 296 | 100% |
| BehaviorControlCenterIIR_c | 7576 | 7565 | 99.85% |

- **MoveTask top-level types:**
  - Attack: 3908, all for players.
  - List: 1416 (the undecoded "0x04" type).
  - Track: 896 (the "0x08" type).
  - ParameterizedBallistic: 43 (the "0x0c" type).
  - KnockUp: 1.
- **List children:** Path 1342, Track 879, Animated 272, GroundAlign 272, Wait 42, Rotate 12.
- **BCC:** the 11 records that are not decoded are EquipItemCommand_c (10) and ObjectInteractionCommand (1). Their command subclass is not decoded.
- **CharDCMove:** all 6752 records are for players. The server never moves an NPC with CharDCMove.

## How the live server moved NPCs

These points are observations from the records. They were not read from the server code.

- **Patrol.** The NPC gets `List[Path home -> first loop point, Path loop (SplinePath loop flag set)]`. The server sends the whole task again about every 1.5 s, starting at the NPC's current position.
  - The loop is closed: the last point joins back to the first.
  - Back-and-forth routes are written out with their points mirrored, for example `[A, B, C, B]`.
- **Chase.** The NPC gets a new `List[Path to the player, Track player]` about every 50 ms. In melee range it gets `Track` alone, together with a `LookAt` on the player and MagicCommand BCC records (the attacks).
- **Smashed.** After a LimboCommand the NPC gets `List[Animated, GroundAlign]`.
- **Monster Laboratory waves.** The waves run a path and then `Track` an NPC (281 Track records have an NPC target), not a player.
- **The "start" field.** It is where the task began. A task the client receives when an NPC comes into view keeps its original start.
- **Speeds.**
  - **Patrol:** only a lower bound is given (straight displacement between re-sends 1–4 s apart). It comes out at about 1.5 m/s, against a walk-speed stat of 2.1.
  - **Chase:** steps between the ~50 ms records came out at 25–45 m/s, far above the run-speed stats. The capture's arrival times for those bursts do not measure movement, so no chase speed is given.

## Notable patrols

| NPC (monster_data) | Zone (visit) | Route (x, z) | Loop length | Speed lower bound |
|---|---|---|---|---|
| Musketeer (192826) ×5 | Pirate World 4600 | 2-point back-and-forth lines, e.g. (89,90)–(95,94), (110,88)–(111,80), (127,77)–(128,68); one is a 3-point line (78,83)–(78,79)–(78,73) | 13–19 m | 1.0–3.4 m/s, median ≈1.5 |
| Musketeer (192826) ×6 | Isle of Yarr (visits 19172, 82537) | the same lines as Pirate World, plus (78,69)–(78,82) and (125,136)–(133,136) | 13–26 m | 1.6–2.6 m/s |
| Jungle Trickster (220120) ×4 | Monkey Cave | one 8-point ring around (327, 251), radius ≈3 m. All four walk it, each starting at a different point | 16 m | ≈1.4 m/s |
| Jungle Trickster (220120) | Isle of Yarr | 9-point loop (307,412)…(319,413)…(310,414) | 37.5 m | 1 sample each |
| Pirate (245481) | Isle of Yarr | (417,168)–(417,175)–(417,180)–(418,175) | 25 m | one record |
| Dragoon (192827) | Isle of Yarr | (182,308)–(190,308) | 15 m | one record |
| Vespid (390750) ×7 | Dinosaur Rise | closed 9–14-point loops, within 3–8 m of home | 18–31 m | one record each |
| Rascal Compy (521911) | Dinosaur Rise | 13-point loop (97,82)…(104,87) | 40 m | one record |

## Other NPC behaviour

- **Stationary:** 189 NPCs never moved. These are mostly invisible hazard and anchor dynels, eggs, pods and guards.
- **Chase only:** 126 NPCs moved only while chasing. They ranged up to 10–20 m from home. Examples:
  - Hunter: up to 17.8 m.
  - Spiderling: up to 17.7 m.
  - Queen's Guard: up to 5 m.
- **Path + chase:** 73 NPCs, mostly Monster Laboratory and Cursed Grove waves. Each ran one path from its spawn point (12–30 m), then chased.
- **Path only:** 25 NPCs ran scripted one-way paths:
  - Pirate prisoners ran off 45–67 m.
  - The Tutorial's Video Game Guy guided the player 196 m.
  - `XX_invis trails runner green` ran up to 97 m.
- **No non-patrol wandering.** No NPC wandered around its home on non-looping paths. Every idle movement in the capture is a looping patrol.
- **Return home after a chase.** 38 chasers were later given a plain path back to within 1.5 m of home (`went_home`), mostly in Monster Laboratory, Cursed Grove and the Tutorial. Most chasers were smashed before any return path appears, or the capture ended first.
