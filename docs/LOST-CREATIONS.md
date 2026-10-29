# Lost Creations (plan and hand-off)

Status (2026-10-28): built (steps 1-3, 5 and parts of 4 and 6; see "Progress" at the end), solo runs
verified in the Linux client; teams not tried with two clients yet.

Sources, in order of authority:
1. **The user's memory of live** (2026-10-28, below as "User rules"). Where the client data or the live
   capture disagreed, the user chose the client/capture (noted).
2. **The live capture** `LMO_Captures.cap`: a solo **Funhouse** run (8000 -> 8060 -> 8030) on the same
   Endless machinery. Decoded in `../research/lmoxide-notes/endless-live-capture.md` (timeline with
   exact records; read it before coding).
3. **The client**: EndlessClientData, quests, playfield dynels, the Flash GUIs and the Linux binary.

## User rules

- Entry: a portal in Dinosaur Rise (6200, door guid **544337** at (217.2, 74.45, 195.89)) opens the
  "Lost Creations" window. Requirement text "Proved your strength at the Gauntlet".
- Teammates, wherever they are, get the invitation window (Cancel / Join). **A Cancel (or no answer)
  aborts the start for the whole team** ("Failed to start..." with the names).
- Levels are picked at random from the finished maps; **Gatekeeper's Realm (7600) on every 5th level
  (5, 10, 15, ...) and never otherwise.** All 11 are picked (2026-10-28; `force_level` pins one for
  testing).
- Start: a red gate and a button (buildie cursor). **Any member** may press it: "Ready to start?" ->
  Start -> 5 s countdown for everyone -> the gate turns green and the 5-minute timer starts.
- Objective done -> next quest step "reach the end", end gate turns green (**Mayhems: end gate always
  green**, one step only). Crossing the end completes the quest **per player**.
- Exit door: every teammate is asked Continue / End Run. **End Run takes only that player out.**
- All minifigures smashed, or time up: that player goes to the **Treasure Chamber (7001)**, is
  **removed from the team by the server** (the client blocks Leave Team inside), and gets **every
  minifigure rebuilt at once**.
- **Minifigures never rebuild inside** a run. **No equipping during a round** (only before the start
  gate opens and after crossing the end).
- **No enemy loot except hearts**; smashables still drop stars.
- **The element changes every 5 levels** (levels 1-5 one element, 6-10 the next: changes on the level
  after each Gatekeeper).
- Rewards in the Treasure Chamber: **minifigure parts** (duplicates possible), **0-2 parts per unlocked
  pool**, a pool unlocked every 2 levels (level 35 -> all 18; updated 2026-10-28). Pools, lowest first:
  [Grandma, Grampa] [Ice Skater, Farmer] [Tribal Woman, Scientist] [Sheriff, Fencer]
  [Lizard Man, Janitor] [Clumsy Guy, Jewel Thief] [Baseball Fielder, Roman Emperor] [Elf, Astronaut]
  [Faun, Ballerina] [Tomahawk Warrior, Hun Warrior] [Robot, Shark Suit Guy] [Frightening Knight, Queen]
  [Unicorn Girl, Skater Girl] [Evil Dwarf, Wrestling Champion] [Flying Warrior, Cowboy]
  [Spartan Warrior, Samurai Warrior] [Laser Mech, Kendo Fighter] [Mr. Gold, Ninja]. They come in the
  Endless Super Chest, dropped at **184.8 39.1 136.4** (the user's) when the player walks into
  `BA_TriggerDropLoot`. Uniform 0-2 per pool, either minifigure of the
  pair, any of its parts: a guess (TUNABLES).
- Levels have variants (enemy themes: medieval, space, mythology, ... even a chicken suit guy wave);
  variants are not one-for-one swaps, but some buildies/smashables are in every variant.
- Also (user, when implementing): **give pocket adventures their elements too** (below).

## Levels (client data)

Finished maps only (the user left out 7102, 7103, 7105, 7106, 7401, 7501 on purpose). Every one has
bounded areas `Start` and `End` (scryid 529106, Linux client RDB 1000053), simple dynels
`GateStartShield` (collision, visual 60626) / `GateStartVisual` (155861 closed red, 155858 open green),
the same for the exit, a `StartButton`, and an exit door (RDB 1000102, `empty_monsterdata_1x1x1`,
monsterdata 465832, scale 3). Guids (shield / visual):

| pf | Map | Start gate | Button | Exit gate | Exit door | Quest (main) |
|---|---|---|---|---|---|---|
| 7100 | Volcano Mayhem | 531704 / 532040 | 532041 | parked / 530186 (closed mesh: show open) | 595118 | 136 |
| 7101 | Castle Mayhem | 538170 / 538169 | 538171 | parked / 538168 | 595119 | 136 |
| 7104 | Candy Mayhem | 538986 / 538985 | 549964 | parked / 538983 | 595122 | 136 |
| 7200 | Swamp Brawl | 532204 / 532203 | 532200 | 530876 / 530877 | 595125 | 138 |
| 7201 | Volcano Brawl | 547843 / 547844 | 547845 | 547841 / 547842 | 595126 | 138 |
| 7202 | Lunar Brawl | 550245 / 550246 | 550247 | 550243 / 550244 | 595127 | 138 |
| 7300 | Valley of the Lost Champions | 538191 / 538190 | 538189 | 538187 / 538188 | 595216 | 139 |
| 7301 | Lost Champions in Space | 547392 / 547391 | 547724 | 547388 / 547389 | 595128 | 139 |
| 7400 | Jungle Arena | 534572 / 534571 | 534570 | 534568 / 534569 | 595129 | 140 |
| 7500 | Isle of Smash | 538209 / 538208 | 538207 | 538210 / 538211 | 595130 | 141 |
| 7600 | Gatekeeper's Realm | 538412 / 538402 | 538413 | 538400 / 538401 | 595131 | 142 |

"Parked": the Mayhem exit shield sits off the map (x 1-2, z 508), so nothing blocks the exit.

- **Start positions** (no resurrection points in these maps; `content/worlds/71xx..76xx` arrival is a
  wrong navmesh guess inside the arena): Candy Mayhem **136.4 40.0 137.2**, Gatekeeper's Realm
  **204.5 42.9 279.3** (user). Others: ask the user, or west of the start gate by the button.
- Gatekeeper spawn (user): 239.9 42.2 277.1 (no anchor in the client data). Gatekeeper variants
  (user): Forest Treeman, Green Drake, Ghost Pirate Grenadier, Red Drake, Crooked Tooth Hurler, Alien
  Mindslicer, Skeleton Mage, Troll Warlord (= "Troll Heavy Hitter War" look). TDB 51000 has 14
  "Gatekeeper" names (2382 .. 2433), a "Lost X" roster (2088-2356) and 2444 "Lost Corrupted Chicken
  Suit Guy" (achievement 3649 "Chicken Surprise").
- Other anchors: 7200 three `invisible_monsterdata` (225.64, 42.99, 267.94), (261.49, 42.99, 257.99),
  (261.85, 42.99, 302.27); 7400 wave points NW_1-4 / NE_1-4 / SE_1-4; 7300 four anchors (champions?).
- **Quests** (main -> step quest/goal): 136 Timed Run 230/274 "Reach the end before the timer runs
  out" (one step); 138 Brawl 232/276 "Defeat enough enemies..." then 239/287 exit; 139 Champions
  233/277 "Defeat 3 Champions..." then 240/289; 140 Survival 234/278 "Survive the assault..." then
  241/291; **141 Smash 235/279 "Smash enough smashies..."** (Isle of Smash; the user had it as
  survival) then 242/293; 142 Gatekeeper 236/280 "Defeat the Gatekeeper" then 237/283. Cooldown 3600 s
  in data: skip cooldowns for these. "NEW OBJECTIVE" is only a HUD banner (TDB 300/165948876), no quest.
- Treasure Chamber 7001: respawn "Treasure Trove" 530344 (141.32, 40.0, 135.89), area
  `BA_TriggerDropLoot` (177.6, 136.4), anchor 549087 (178.92, 41.42, 136.59) (chest spot?), exit door
  530333 -> window 70 -> 6200 `TP_FromLostCreation`. Chests 531717 "XX_Endless Super Chest" / 531718
  member (content/items marks them `unused`); buff 530365 after the loot drop.

## Client protocol

GameCharInterfaceProject (server -> client) / GameCharAgentProject (client -> server); ids in
data/rpc_methods.json. Texts: **EndlessClientData RDB 1000647 id 1** (Lost Creations; 654716 is the
Funhouse) — "Lost Creations", "How far can you go?", Continue/End Run, splash texts, Cancel/Join.

| Step | Server | Client |
|---|---|---|
| Entry / end-portal / invite window | 0x1d `ScryShowEndlessPlayfieldsTeleportMessageBox(u32 type, u32 required_level, String quest_hash, i32 ctx, u32 time_left, u32 proto_id=1)`; type 0 entry, 1 continue (the player at the door), 2 invitation, 3 continue for teammates | GCA 31 `...MessageBoxAnswer(answer, ctx)`: entry Enter 1 / X -1; continue Continue 1 / End Run 2 / X -1. GCA 32 `EndlessPlayfieldsInvitationAnswer(n)`: invite Join 2 / Cancel 1; type 3 Continue 2 / End Run 1 / close 0. A timed-out window sends nothing. A new window makes the client answer -2 for the old ctx |
| countdown text in the window | 0x24 `SignalStartEndlessPlayfieldsInvitation(u32 seconds)` | |
| close | 0x1e `ScryCloseEndlessPlayfieldsTeleportMessageBox()` | |
| someone declined | 0x28 `SignalEndlessAborted(GroupVec_t declined, GroupVec_t)` (names from the first) | |
| "wait until all cross the finish line" | 0x1f `EndlessPlayfieldsInvitationFailed()` | |
| "Ready to start?" | 0x26 `SignalOpenEndlessWarmupDialog()` | Start: GCA 34 `StartEndlessWarmup()` (red X: nothing) |
| "GATE OPENS IN" 5..1, GO! | 0x25 `SignalStartEndlessWarmup(5)` (client counts) | |
| HUD quit button | | GCA 33 `LeaveEndlessPlayfields()` (after its own confirm) |
| teammate busy | CSPlayerInterface 0xc2 `TeamInviteTargetInEndlessPlayfield(GroupElement_c)` | |

- String encoding seen live: the entry window's quest hash was u16 1 + NUL, the continue window's
  u16 0. Use the requirement quest hash in type 0 (the client checks HasEverCompletedQuest; our journal
  marks every story quest done, so any Gauntlet quest hash passes; pick the Gauntlet's).
- **PlayfieldProxyData** on entering a level: `endless_progress` = level (1, 2, ...), `identifier` =
  run id (same all run), `team_size`, `elemental_type` = the block's element,
  **`client_protobuf_resource_id` = 1** (else splash texts are empty), `endless_protobuf_id` /
  `endless_protobuf_resource_id` (live: per-level ids / 654269; server data, probably unused by the
  client). `endless_progress != 0` is the client's EndlessState: HUD level badge + quit button,
  "endless" quest icon and popup, story quests hidden, team invite/leave blocked, death splash.
- **Timer HUD** (`EndlessPlayfieldsTimerHUD.swf`, always loaded in play): a clock top right by the
  minimap, shown while the player has buff `GetGameTweak("EndlessPlayfieldsTimerBuff")` = **521912**
  (client built-in default, RDBGameTweaks::SetDefaults: no override needed; live's OverrideGameTweaks
  was empty). mm:ss from the buff's remaining time, warning at <= 5 s, dungeon music; badge = level.
  Buff runs out (or removed with < 1 s left) -> "Time's Up!" splash with "{level-1} Lost Creations
  Completed!"; removed with time left -> clock closes quietly. Local death with EndlessState -> "You
  were smashed!" splash. Playfield change closes it.

## Live sequence (Funhouse capture; replicate)

1. Entry: type 0 -> answer 1 -> close, TeleportStarted, IntendedTeleport -> a new copy of the level.
2. At CharInPlay: buff **537829 CantResurrectWeapons**, duration 2,000,000 cs, once per run (later
   levels re-add it with the time left); the level's quest (MainQuestUpdate + QuestFullUpdate); stat
   ElementalType.
3. Button = simple dynel **"XX_Interactable"** (51320, remoteformat 482763) with an **"XX_InvisibleDummy"
   NPC** (cars_group 5) over it carrying buff **577484 Button Arrow**. ObjectInteraction (18) on the
   button -> 0x26. GCA 34 -> 0x25(5) at once, Button Arrow removed, EffectPackage 474576 on the button.
4. +5 s: timer buff 521912 **add with 20000 cs then update (mode 2) to 30000 cs** (300 s); start gate
   visual `SimpleDynelVisualChangeIIR_c` state 3 visual 155858 (+ StatIIR stat 12 = 155858), shield
   state 4 visual 0 (collision off). +1 s: buff **537723 CantEquipWeapons** (2,000,000 cs).
5. Goal progress (QuestGoalIIR); objective done -> next step's QuestFullUpdate, exit gate opens the
   same way.
6. Crossing End: quest done (QuestIIR 2, Journal, QuestIIR 4, MainQuestUpdate remove); **timer and
   CantEquip removed**.
7. ObjectInteraction on the exit door -> type 1 at once -> answer 1 -> next level (new copy).

Not in the capture (solo run, ended in the Funhouse finale): invitations, time-out, wipe, Treasure
Chamber. Follow the client and the user rules there.

## Elements

Ids: **1 Light, 2 Shadow, 3 Tech, 4 Magic, 5 Nature** (content/minifigures `element` agrees; the
client lists them reversed). Sent as PlayfieldProxyData `elemental_type` and the player's stat
**ElementalType 3000109** (StatIIR_t + StatUpdateIIR_c after CharInPlay, every zone entry; live sent
0 too). The minimap ring (Minimap.swf `m_ElementalFrame`) shows it, brighter when the active
minifigure matches. LMOxide sends neither today.

Per world: Linux client RDB **1000644/1** teleport-window entries `elemental_type` (read with
`rdb.index("../clients/linux/LEGO Minifigures Online")`; the clients/windows copy is older and lacks
it). Side dungeons all have one, plus 2500 Shadow, 3500 Tech, 4500 Nature, 5500 Magic; open worlds,
the hub, Winter Pass and the type-8 adventures (Medusa, Armory, Underworld, Space HQ, Shadow
Centurion, Pirate Trials/Grotto, Ruined Fort, Sorcerer's Keep, Gauntlet) have none. Full list:
research note.

**Portal doors**: the last u32 of `DoorFullUpdateIIR_c` (after u32 stat 169) is the destination's
element (DoorFullUpdateIIR_c::ReadSubClass +0x1ac -> door stat 3000109 -> map marker element; very
likely the portal glow). Live sent it on every dungeon door (all 14 seen match the table); teleport.rs
`door_records` sends 0 (its "13 zero bytes" tail). Verify the glow colours in 4200 after the fix.

## Data model (content)

- `World.element: Option<Element>` (copied from the client table; editable). Kind of 71xx-76xx and 7001:
  `instance` (they're `other` now).
- `Spawn.variants: Vec<String>`: empty = in every variant (shared buildies/smashables), else only in
  those. A level picks one variant at start. `Script.variants` (exists, unused by the server) keyed by
  the same ids, so the variant picks the script version too; `when: {lc_level: [a, b]}` works as is.
- New record `lc_level` (`worlds/<world>/lost_creations.json`): `mode` (mayhem | brawl | champions |
  survival | smash | gatekeeper), `start` {pos, rot}, gate guids (start/exit shield + visual),
  `button`, `exit_door`, quests (objective, exit), goal (defeat n, smash n, survive waves, champions 3),
  `variants` [{id, name, weight, when}], gatekeeper spawn.
- New record for the run (`content/lost_creations.json` or under content/rules): entry door 544337,
  requirement quest hash, times (level 300 s, warmup 5 s, invitation and continue answer seconds:
  unknown, pick and log), gatekeeper every 5, element block 5, Treasure Chamber + its position,
  enemy loot table (hearts only), reward pools (slugs) + 2 levels per pool + 0..2 parts, the reward
  chest and its spot, `force_level` (testing only; wins over the Gatekeeper rule).

## Server design

New `lost_creations.rs`: runs keyed by team (a solo player is a team of one). Run = members, level n,
current copy (`Pf`), phase, element, deadline, answers. Phases: gathering (entry window, invites,
answers) -> waiting (gates closed, button live) -> countdown (5 s) -> running (timer, objective) ->
exit open -> finished (members cross End) -> continue question -> next level | out.

- **Copies**: `instance::new_copy(model, mode)` for each level, teleport the team there with that exact
  `Pf`. Don't go through `instance::arrival`: its "copy you left in the last 3 minutes" and "teammate's
  copy" rules would put players back into an old level.
- Hooks needed: ObjectInteraction on door 544337 (6200), on the StartButton, on exit doors (game.rs
  interaction chain, as teleport doors); GCA 31-34 in game.rs; area enter for `End` (areas exist in the
  Linux RDB; content has no 7xxx areas yet: import them); smash.rs/game.rs: no rebuild while in a run
  (CantResurrect) and the wipe -> out; equip refusal while CantEquip; quests.rs: start/complete a quest
  step by id (not by area), no cooldown for 136-142; loot.rs: enemy loot = hearts only in run copies;
  community.rs: remove from team (server side), invites blocked (0xc2); teleport.rs: playfield proxy
  endless fields + elemental_type, door element tail.
- Out of a run (End Run, time up, wipe): teleport to 7001 (+ "Treasure Trove"), remove from team,
  unlock every smashed minifigure (UnlockTemplate), drop the run buffs, rewards at the chest.
  Time up is the client's own splash (buff expiry); the server must also act at the deadline.
- Achievements: `Event::LostCreations { level }` exists (achievements.rs, `!lc`): fire it per player on
  each completed level; 4701/4703/4702 (levels 25/30/35) and 3649 Chicken Surprise have no rule yet.
- Elements for all worlds (proxy + stat + door tail) can go first: small, testable in 4200 / 1640.

## Steps (each ends verified in the Linux client)

1. Elements: World.element imported from RDB 1000644; proxy `elemental_type`, ElementalType stat on
   entry, door tail. Check the minimap ring in 1640 (Magic) and portal glows in 4200.
2. Content: 7xxx areas (Start/End) + gate/button/door spawns, `lc_level` for 7104 and 7600, run record
   with the Candy override, start positions.
3. Run solo: entry window -> copy of 7104 -> button -> warmup -> gate -> timer -> End -> continue ->
   next copy; time-out and wipe -> 7001.
4. Teams: invitations, abort, per-player End Run, continue questions (types 1/3, 0x1f), server-side
   team removal.
5. Rules: no rebuild, no equip mid-round, hearts-only loot, element blocks, Gatekeeper every 5th
   (with the override off via a command).
6. Rewards in 7001; achievements; the other levels' content once the user has it.

Test commands to add (devcmd.rs, README table): `!goto lc-entry`, `!lc start [level] [map]`,
`!lc objective` (complete the objective), `!lc time <s>` (set the timer), `!lc end` (leave), keep
`!lc <n>` for achievements. Use the LAN server and the Linux test client (memory notes).

## Open

- Invitation / continue answer seconds (not in the capture; pick and log in TUNABLES).
- Start positions of the levels other than 7104 / 7600; enemy rosters and counts per level/variant.
- What `LockTemplate(t, 0 s, flags 0x10)` for ~50 templates at 1323 s in the capture meant.
- The Treasure Chamber flow on live (chest spawn, buff 530365, how rewards were handed out).

## Progress

Built 2026-10-28 (lost_creations.rs; content `lc` records in content/lost-creations, tunables
`lost_creations`; numbers in docs/TUNABLES.md "Lost Creations"):

1. Elements: world `element` (tools/re/extract_elements.py: 33 worlds), PlayfieldProxyData
   `elemental_type`, stat ElementalType after every CharInPlay (StatIIR_t + StatUpdateIIR_c), the
   door tail. Verified: Natives' Hollow's minimap ring is Magic; the portal to it in Isle of Yarr
   glows purple (not compared with the old build).
2. Content: tools/re/extract_lost_creations.py writes the 11 levels' gates, buttons, exit doors,
   Start/End areas and `lc` records (kind `instance`), the Treasure Chamber's door and areas;
   the Smash and Gatekeeper quests' steps are in the client's order now (objective first).
3. Solo run, verified in the Linux client: entry door -> window (client texts, requirement quest
   81HI "Test your mettle inside the Gauntlet") -> new copy of Candy Mayhem (red gate, button
   arrow, quest, quit button, no-rebuild icons, element ring) -> E on the button -> "Ready to
   start?" -> countdown -> green gate, clock with the level badge -> End: "QUEST COMPLETED", clock
   gone, achievement event -> exit door -> "You completed level 1!" Continue -> level 2 in a new
   copy; time up ("Time's Up!"), all minifigures smashed ("You were smashed!"), the HUD quit and
   `!lc end` -> Treasure Chamber, every minifigure rebuilt; rewards drop when walking into
   BA_TriggerDropLoot ("Grandma Body"). A reconnect into a level outside a run -> Treasure Chamber.
4. Teams: built, NOT tried with two clients: invitations (type 2 + 0x24), Cancel / no answer ->
   SignalEndlessAborted to everyone, exit door: 0x1f until everyone is across, type 1 / type 3,
   End Run only that player, server-side team removal, team invites into or out of a run refused
   (0xc2). The invitation/continue windows' `time_left` is sent as seconds (a guess).
5. Rules: no rebuild inside, no equipping while the timer runs (refused EquipItemCommand; the
   CantEquip buff), hearts-only enemy loot, element blocks, Gatekeeper every 5th (off while
   `force_level` is set). Equip refusal not tried in the client.
6. Rewards in 7001 done; achievements: Event::LostCreations per completed level. Not done: the
   other levels' enemies (their worlds have no spawns: a run there is a walk to the end), the
   Gatekeeper's spawn (lc `gatekeepers` empty: no NPC records known), level variants (the model
   and npc.rs filter exist, no level has any), achievements 4701/4703/4702 and 3649.

Open beyond the list above: mouse clicks on the start button didn't register when injected (E
works; the user should try clicking); the HUD portrait after the wipe showed the first slot's
minifigure while the character wore the third's (the old teleport portrait desync?).

Changes 2026-10-28 (user's list): every level picked (no enemies in any yet: only the Mayhems can be
finished); 18 reward pools, one per 2 levels; the rewards in the Endless Super Chest at 184.8 39.1
136.4; "Ready to start?" not re-sent for 10 s (the client stacked them, the X couldn't close the
older ones); the entry window's X closes it; a disconnect keeps the player in the run for 180 s
(the timer runs on) and a login within that puts them back in the level's copy where they were.
