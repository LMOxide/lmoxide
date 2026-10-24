# LMO client protocol notes

Findings for the shipped client (`Version.txt`: `LMOv1.5@557666`, built 2016-05-23). LMO runs on
Funcom's Dreamworld/N3 engine (the Anarchy Online / Age of Conan / The Secret World lineage),
not on LEGO Universe's engine, so DLU and the LUDevNet docs don't apply.

Status legend: **verified** = observed working against the real client; **static** = read from
the binary only.

## Boot sequence

1. `Launcher.exe` (.NET) reads `ControlHttpPatchAddr` from `LocalConfig.xml`, downloads
   `<control>/lmo_live/LocalConfig.xml` and `PatchInfoClient.txt`, then runs `LMO.exe -verified`.
   We skip the launcher: write `LocalConfig.xml` into the client folder and run `LMO.exe -verified`
   directly (see `tools/dev-run.sh`). **verified**
2. The in-client patcher (`CorePatcher`) fetches `LocalConfig.xml` and `PatchInfoClient.txt` again,
   then walks: GetClientFileList → GetDownloadList → GetRDBHashIndex → PatchRDBHashIndex →
   PatchPacks → ProcessDownloadList → WaitingForSubstanceCaching → PatchingDone. **verified**
3. The login window is a CEF page loaded from `LoginURL`. The page calls `user.login(a, b)`
   (injected by `DWWebHelper.exe`). Alternatively pass `-username X -password Y` on the command
   line for automatic login (`charslot`, `charname` also exist). **verified (command line)**
4. Universe agent (`UniverseAddr`, TCP) → territory manager (PlayerAgent) → game server.

## HTTP patch layout

Relative to `HttpPatchAddr`:

| Path | Content |
|---|---|
| `rdb/full/<md5>` | content-addressed file named by `PatchInfoClient.txt` hashes (download lists, `RDBHashIndex.bin`) |
| `rdb/res/<h[0:2]>/<h[2]>/<h[3:]>` | one RDB resource; `h` = md5 of the *uncompressed* resource (also its local file name, `RDB/<type>/<HASH>.rdbres`) |
| `client/<h[0:2]>/<h[2:]>` | client files from `FileList.bin` (94 shader-cache files missing from the archive; failures are non-fatal) |

- Downloads may be compressed: bodies starting with magic `IOz1`, `IOz2` or `IOg1` are decompressed
  (zlib/LZMA); anything else is stored as-is (`0x48e830`). We serve raw files. **verified**
- `PatchInfoClient.txt` → `DownloadList-CLIENT-Windows32-SSE2DXT-modelQ0=<md5>` selects the
  platform download list. If the local `RDB/DownloadList-Platform.xml` has a different md5 the
  client downloads `rdb/full/<md5>`.
- The archived client lacks ~4,065 listed resources (~430 MB, mostly rdbtype 1010001/1010180).
  The server serves a filtered list. The client rejects it unless counters are consistent:
  per `<ResourceList>`, `ResourceCount` = entries, `DownloadSize` = Σ `size`, `SubstanceCount` =
  entries with rdbtype 1010061; on `<DownloadList>`, `ResourceCount`/`DownloadSize` count unique
  hashes. **verified**
- Funnel tracker: `GET <FunnelTrackerAddr>?category=..&platform=..&action=..`; any 200 is fine.

## TCP framing **verified**

```
frame   = u32be len, u32be crc32(body), body        ; len = 4 + body.len()
body    = u32be hlen, RpcHeader (protobuf, hlen bytes), args
```

If bit 31 of `len` is set the frame is a control frame: same crc32 + body layout, body is not
an RPC. The client sends one with body `01` right after game-server authentication (meaning not
yet known; ignoring it works).

`RpcHeader`/`ProtoEndpoint` are in `proto/Util.proto`. `endpoint_id.type` and `caller_id.type`
are a Java-style hash (`h = h*31 + c`) of the endpoint name (e.g. `UniverseInterface` →
`0x0c384054`). `caller_id.instance` carries an identity some handlers check.

## Method IDs

Extracted for every interface by `tools/re/extract_rpc.py` → `data/rpc_methods.json` → generated
constants `lmoxide_net::methods::<interface>::<METHOD>`. Most interfaces use 32-bit hashes; some use
declaration-order indices (`UniverseAgent`, `ClientToGameserverRPC`, `GameserverToClientRPC`).
Receivers (server→client) come from each `*Receiver::DispatchMethod` switch; senders
(client→server) from the index passed to `rpc::Proxy_t::SendMethodInvocation`.

## Argument encoding (BinaryStream) **verified**

Big-endian. `std::string`/`String`: u16 length + bytes (length ≥ 0x8000: `ffff 01ff` + u32).
`ID32`: u32 type, u32 instance. `ID32::Instance`: u32. `bool`: u8.
Containers (vector/map): u32 `(n + 1) * 1009`, then the elements (map: key, value).
Embedded protobuf (e.g. `FidData_c`, `PlayfieldProxy`): u32 length + encoded message.

## Login flow

| Step | Call | Notes |
|---|---|---|
| 1 | → `UniverseAgent::InitiateAuthentication(username, gameID=1)` | **verified** |
| 2 | ← `UniverseInterface::Challenge(data)` | we send 32 `0`s; the client accepts any |
| 3 | → `UniverseAgent::AnswerChallenge(answer)` | 641 chars `"<hex>-<hex>"` (Diffie-Hellman); not verified by us |
| 4 | ← `UniverseInterface::AckAuthenticate(nAuthStatus, playerID, territoryAddr, cookie, eReason)` | handler `0xb4b580`: `nAuthStatus` 1 = OK; `eReason` must be 0 (non-zero is shown as a login problem, e.g. `e_NoGameServers`); **caller instance must equal `playerID.instance`** **verified** |
| 5 | → `PlayerAgent::Authenticate(account, cookie, territoryVersion=0)` on the territory address | **verified** |
| 6 | ← `PlayerInterface::AuthenticateAck(nStatus=1)` | client proceeds **verified** |
| 7 | → `PlayerAgent::GetStartupData(md5)` | **verified** |
| 8 | ← `UpdateClientPlayerData`, `SetDimensionList`, `PlayerSetupComplete(1)` | see encodings below; the client's GameClient state goes to 7 ("idle") and it drops the territory connection after 30 s of inactivity (timer at `0xb53c9d`) — normal |
| 9 | ← `VerifyClientStillConnected` every 10 s → `ClientReplyKeepCookie` | heartbeat |
| 10 | (UI) launcher shows START; `flow.EnterGame()` = pressing it | `LMO_AUTOPLAY=1` does this from Lua |
| 11 | → `PlayerAgent::CreateCharacterWithData(dim, StrStrMap, u16 region, u16 age, u16 gender, igrID, u32 userType, u32 serviceType)` | sent when the account has no characters; data map empty, igrID `"_"` |
| 12 | ← `PlayerInterface::LoginConnectReady(addr, cookie, ID32 char, PlayfieldProxy)` | decoder `0x7bf400`; we use char type 50000, playfield 1200 |
| 13 | → game server: `ClientToGameserverRPC::Authenticate2(charInstance, cookie, "LMOv1.5@557666_Win32")` | ← `GameserverToClientRPC::AckAuthenticate(1)` |
| 14 | → control frame `01`, `ClientToGameserverRPC::Ping(seq)` (← `Pong`), `GameCharAgent::Ping(seq)` every ~1 s | client FlowControl state `WaitingForFullUpdate` |
| 15 | ← `GameserverToClientRPC::PassBlob` with `PlayfieldFIIR_t` (playfield 51100:1200) | client loads the playfield ("Entering playfield" screen) **verified** |
| 16 | (wait until the account's territory connection has closed — see below) | |
| 17 | ← `UpdateMessage` (create dw::GameObject), `SimpleCharFullUpdateIIR_t`, `FullPlayerCharacterIIR` | character dynel created; FlowControl → WaitingToSendInPlay **verified** |
| 18 | → `ClientToGameserverRPC::SignalPassBlob` with `CharInPlayIIR_t` ← echo `CharInPlayIIR_t` | FlowControl → **InPlay**, world rendered **verified** ([screenshot](img/first-inplay.png)) |
| 19 | → client IIRs: `CharDCMoveIIR_t` (position), `MoveTaskIIR_c`, `QuestCooldownIIR`; GameCharAgent requests (item shop, gear, guild, tutorials) | **next**: appearance (minifigure), stats, movement echo, the GameChar RPCs |

## IIRs (state updates)

`GameserverToClientRPC::PassBlob(BinaryStream)` = u32 length + a sequence of IIR records,
processed by `n3Engine::AddNetWorkMessage` (`0x7d4540`). Record (`0x7d6b20`):

```
u32   type id = Java string hash of the registered name     (data/iir_types.json, 117 names)
ID32  identity (the dynel/playfield the IIR is about)
bool  isToBePassedOn
[u32 len + protobuf]  if the class has a header message (GameCodeDynel IIRs: ProtoGameCodeDynelIIR)
...   ReadSubClass (vtable +0x20)
```

The engine calls `PollStatus` (vtable +4): 1 → `Activate` (+8), 2 → queue and retry each frame,
3 → drop (`0x7d56e0`). Builders live in `lmoxide_net::iir`; layouts are documented next to each
builder in `crates/lmoxide-server/src/game.rs`.

Pitfalls found:
- Reading from an empty `BinaryStream` yields an uninitialised value. Every blob/list the
  client reads needs at least its count (`PlayfieldFIIR_t` blob 2 empty → out-of-memory dialog).
- Playfield ID32 type is 51100 (key of the PlayfieldFactory map, `0x96cee0`).
  `PlayfieldProxyData.running_instance` is the key of the client's playfield map.
- Character stats: the HUD (Flash `GetStat`, native `0x821eb0`) reads the dynel's `StatHolder_t`
  (+0xac). `Int32StatIIR` (u32 stat, u32 value; applied by `0x87b270` → StatHolder SetStat) sets
  them; ids are the engine's `stats::e_*` (data/stats.json: Life 1 = max health, Health 27).
  Neither the proto `CharacterStats` nor the full update's two (u32,u32) maps (+0x14c/+0x154) feed it.
- `CoreCharacterFullUpdateIIR` asserts `generic_stats_size == g_Int32StatsOrder.size()` (= 1).

### Dynel creation (solved)

Order: `UpdateMessage` creates a DreamWorld GameObject; then the dynel IIR (`SimpleCharFullUpdateIIR_t`)
whose PollStatus waits for that GameObject creates the dynel through `SimpleCharFactory`.

`UpdateMessage` (`dw::UpdateMessage`, ReadSubClass `0x6ef680`): u8 op (0 = create), ID32,
u32 playfield key, blob. The blob is **little-endian** DreamWorld reflection data (`0x6ef090`):
an object is `u32 byte length` + (`u32 field id`, value)*, field id = ELF/PJW hash of the field
name (`0x4ab4a0`). Struct-typed fields nest the same way; primitive u32 fields are 4 LE bytes.
`dw::GameObject` fields (`0x6f3a10`): `Id` (ID32 = struct {type @0, instance @4}, `0x6f3e80`),
`Position`, `Rotation`, `Scale` (Vector3Member/quaternion structs). Only `Id` is needed.

Territory race: the client drops its territory connection when it moves to the game server, and
FlowControl (`0x410290`) logs out if that disconnect is noticed in any state but 1/3/4/5/7
(7 = WaitingForFullUpdate). The game server therefore waits for the account's territory
connection to close (`sessions.rs`) before sending the world.

### Dynel creation — original notes

Character IIRs (`SimpleCharFullUpdateIIR_t`, type check 50000) derive from
`GameCodeDynelFullUpdateIIR`. Their `PollStatus` (`0x7442f0`) returns 1 only when a
`dw::GameObject` exists for the identity (map at `0x1ef76e0`, value at +0x18); `Activate`
(`0x743fc0`) then creates the dynel through the dynel factory map (`0x1ef7994`; `SimpleCharFactory`
is registered for 50000 in `0xde1b80`), and `n3DynelFactory` (`0x7d3110`) asserts the GameObject
exists. So something must create the GameObject first — not yet found. Candidates creating
GameObjects: `0x6e4960`/`0x6e4f80`/`0x6e5520`/`0x6e5ae0` (DreamWorld playfield modules, called
from `0x9e0860`…), `0xdf2d80` (game code), `0x6ea4c0`/`0x6fce20` (via `0x6fab80`). The playfield
IIR's blob 1 (nested objects created through the map at `0x1ef7a08`) is another candidate.

### Characters in the world

- **Stats** (HUD health bar etc.) live in the dynel's `StatHolder_t`; `Int32StatIIR` (u32 stat,
  u32 value) sets them. Ids: data/stats.json (Life 1 = max health, Health 27, Level 54,
  MonsterData 359, GmLevel 215).
- **Body and costume** come from `CharacterStats.monster_data` (plus `monster_scale` 100):
  each minifigure item template has a SetStatGameAction MonsterData → its own monsterdata
  (Astronaut 663929 → 663910; table in game.rs). 2212 in the templates' stat block is only the
  generic body ("Aztec_Warrior"). Without monster_data remote characters are invisible.
- **Other characters** need `UpdateMessage` (GameObject) + `SimpleCharFullUpdateIIR_t` + stats +
  `CharInPlayIIR_t`. Despawn: `UpdateMessage` op 1 (u8 1, ID32).
- **Movement relay**: the client sends `MoveTaskIIR_c` for click-to-move (u8 task type 0, start
  pos, u8 0x0f, 10 zero bytes, f32, u32 n, n waypoints, 16 zero bytes) and `CharDCMoveIIR_t`
  for keyboard movement (u8 flags, pos, ...). Relaying MoveTaskIIR_c with the mover's identity
  makes remote characters walk. Move tasks are read by a factory (u8 type → creator table) and
  validated (`IsValid`).
- **Keyboard movement** needs stat CharRadius (421): the collision capsule radius is
  CharRadius × MonsterScale / 10000, and with 0 `Vehicle_t::CollideAndSlide` blocks every step.
  CommandQueueSize (1360) defaults to 1, which makes the queue "full".
- **Chat** (chat.rs): free-text chat needs GmLevel ≠ 0 (ChatGUI criteria `stat:gmlevel!=0`).
  The chat server is plain IRC (CRLF lines, zlib unless LocalConfig `ChatCompression=false`) at
  the address from `PlayerInterface::ChatServerConnectReady(String "host:port", u32 cookie,
  ID32)`, sent before LoginConnectReady. Client: `PASS :<cookie>`, `NICK <name>-<PlayerID>`,
  `USER ...`; server: `001`, and `JOIN #ChatGroup_General_D1_1` (the window's default "Global"
  tab needs a joined `#<group>_D<dim>_<n>` channel). Before sending, the client asks a
  hard-coded filter URL (`http://chatfilter.funcom.com/rest/chat_filter?...`; patched to our
  `/rest/chat_filter` by patches/patch-client.py) and sends only if it answers
  `{"result":"0","filtered":"..."}`. Channel lines then arrive as `FILTEREDPRIVMSG <target>
  [<hash>] :<text>` and are relayed as `PRIVMSG`; "Say" goes to the game server
  (`GameCharAgent::VicinityChatMessage`) and is shown to others via IRC `VICINITYCHAT`.
- **Client file patches** survive the patcher because the HTTP server rebuilds FileList.bin
  (RootHash) and PatchVersion from the local files (see http.rs `rebuild_file_list`).

## Friends and teams (community.rs)

The community server's RPCs travel on the game connection (`CSPlayerAgent` →, `CSPlayerInterface`
←, endpoint instance = character). Addresses are the Linux build's. Character ID32 type 50000,
team 57001 (raid 57016).

- `GroupElement_c` = u32 length + `GroupElementData` (proto). The client uses
  character_instance, name, dimension, playfield and member_slot_index (0..3: list / map marker
  colour). `TeamInfo_c` (0x197c236) = ID32 team, ID32 leader, vector<GroupElement_c>, u32 loot
  mode, u8 need/greed, u8, ID32 master looter, u32 loot threshold. `Friends` = u32 length +
  `FriendsData`. `RemoteFormat` = one String.
- **Friend codes** (Social window): `GameCharAgentProject` 27 RequestMyFriendKey →
  `GameCharInterfaceProject` 0x13 RequestMyFriendKeyReply(String "XXX-XXX-XXX-XXX") (recorded
  live); 28 RequestAddFriendWithKey(String) → 0x14 RequestAddFriendWithKeyReply(u32: the
  friend's character instance, 0 = not found), after which the client sends CSPA 105
  SignalAddFriendByID. Our codes encode the instance (community.rs `friend_code`).
- **Friends list**: the client never asks; 0xc3 SignalFriendsListUpdate is an upsert per friend
  (online status, playfield proxy); 0xc4 SignalFriendsRemoved (entries with only the instance).
  0xbe FriendInviteFrom(GroupElement) → Yes/No → CSPA 102/103. Remove: CSPA 108.
- **Teams**: CSPA 3 TeamInvite(ID32, string, bool) → invitee 0x01 TeamInvite(GroupElement,
  string) (Yes/No dialog), inviter 0x0c YouInvitedToTeam; CSPA 8/9 Accept/DeclineTeamInvite(ID32
  inviter). **0x10 YouJoinedTeam(TeamInfo with every member, bool feedback)** is what puts a
  client in a team; others get 0x16 TeamJoin(team, GroupElement, bool). Leave and kick are
  `GameCharAgent` 9 LeaveTeam() / 10 KickFromTeam(ID32) → 0x15 TeamLeave(team, GroupElement,
  bool) to everyone (0x25 YouGotKickedFromTeam only shows text), 0x18 TeamDisband(team) when one
  member remains (the client also leaves by itself then). Leader: CSPA 14 → 0x19
  SetNewTeamLeader(team, GroupElement, bool). 0xbd TeamMemberLinkDeadUpdate(ID32, bool **online**).
  Live sent 0x02 YouAreNotInTeam at every playfield entry.
- On the game server: `AddToGroupIIR_c` (ID32 group, ID32 leader) / `RemoveFromGroupIIR_c`
  (ID32 group) on a character, or ProtoCoreCharacterIIR team_id / team_leader_id in its full
  update, set the team the client compares in `SimpleChar_t::SameTeamAsClient`. Map markers:
  GCA 125 RequestTeamMemberPosList(vector<u32>) → GameCharInterface 0x7c
  UpdateTeamMemberPosList(vector<{u32 instance, i32 x, y, z}>).

## Loot (loot.rs)

- Only the bag's **owner** sees its items (`ProjectLootBag::OnEnteringPlayfield`), so team loot
  is one bag per member. After each bag live sent `NewAllowedLooterIIR_c` (ID32). CanLoot
  (0x122bb2a): allowed looter 0 = anyone, a team id = its members, a character = that one.
- **Minifigure parts** (RDB 1000645 id 1 `MinifigurePartsData`, data/minifig_parts.json): three
  per minifigure, owned as **tokens** (UpdateTokenAmount gained/spent; token 1SSNN head, 2SSNN
  torso, 3SSNN legs). The part item (dynel type 34002) is clicked, not auto-looted.
- **Room check**: ClientLootBag::CanLoot refuses an item ("InventoryFull", no TryLootItem) when
  the inventory it would go to (`ItemHolder::GetItemTypeInventory`) has no free slot: 34001 bags
  -> 110002, static-template items (stars) -> none, everything else incl. parts -> backpack 104
  (`ItemHolder+0x50`, created empty by CoreItemHolder::Initialize). Live's FullPlayerCharacterIIR
  carried 101 (32), 104 (50), 112 (48), 113 (48), 120 (20), 119 (24), 110001 (160), 110002 (200).
- **Open question:** parts are picked up with the interact key (E). A mouse click on a part
  casts the basic attack instead: `SimpleDynel::CanOpen` needs `CanBeOpened` (+0x33a; set by
  SetCanBeOpened, UpdateSimpleDynelIIR_c::Activate, the CoreLootBag ctor) and not IsClosed.
  Whether live allowed clicking is unknown; revisit if it turns out it did.
  `GameCharAgentProject` 0x15 ConvertMinifigureParts() → 0x22 response (1 = done); 0x16
  BuildminifigureFromParts(u32 template) → 0x23 response (0 = built).
- **Minifigure bags** (dynel type 34001, one per minifigure, data/loot_items.json; inventory
  110002, sent in FullPlayerCharacterIIR, max 200): on pickup `GameCharInterface` 0x24
  AddItem(ID32 (110002, char), i32 pos -1, entry, u8 0) starts the reveal (pos -1 isn't stored);
  then AddItem into 110001 for a new minifigure, or stars for a duplicate.
- **Chests** are items in a bag with stat LootType 1 (free) / 2 (member): the client shows the
  chest model at the bag + Loot_Offset; clicking sends TryLootItem; the server drops the chest's
  contents as a new bag. The LootTable contents were server-side.

## Buildies (buildie.rs)

Buildies are `DestructibleFullUpdateIIR_c` dynels of type 51321 with CARS group 1 (the pirate
tutorial's ramp, wind propeller, mortar and cannons; recs 6909-29358 of the recorded session).
Building, as live did it:

1. Client: `LookAtIIR_t` at the buildie + `BehaviorControlCenterIIR_c` action 3, CommandType 18,
   CommandMessage {1: 686, 2: client id, 5: 10, 6: 0, 10: player ID32}, data = buildie ID32 + u32 0.
   Server: echoes the LookAt and answers action 4 (success, field 3 = server command id).
2. On the buildie: `FloatStatIIR` 2999049 = summed build rate per tick (player 62.0, a
   scripted NPC 19.0, both 81.0); `ScryScriptIIR_c` "buildableStart"; `StatIIR_t` 2999047 = 1.
3. Every tick `StatIIR_t` 27 (progress) += rate, capped at the maximum (1600; one cannon 1100).
4. Done: stat 27 = max (sent twice), stat 455 = 58, `SimpleDynelVisualChangeIIR_c` (4, built
   mesh), ScryScript "buildableStop", VisualChange (2, 0), stat 1050 (CARS group) = 5 (twice),
   FloatStat 2999049 = 0, `UpdateSimpleDynelIIR_c` 00000000 01010001, stat 1050 = 5.
5. Commands the client queues while a build runs get action 4 then 5 (failure, reason 21).
   After a stop (player left), restarting reset 2999047 and stat 27 to 0 first.

The tick interval isn't recoverable (the recording keeps only the microsecond part of times).
A full update of a built buildie (sent to newcomers) has SimpleDynel field 1 = the built visual,
CARS group 5, no particles, Health = max and the Destructible's 4th field = 1. The jump pad
under a buildie (a separate `Jumper` dynel) is in CARS group 5 until it is built, then 3.

## Levers, gates, pressure plates (scenery.rs: shown, not functional)

- They are playfield simple dynels (RDB 1000057): `lego_lever_a_interactable` (Space Colony 3200 ×5,
  Gauntlet 6300 ×2), `module_wall_4x4_gate_lego_myth_interactable` (6300, Medusa's Lair 2300, 4610),
  `pressureplate_red/yellow/green` (Ruined Fort 5300, 9 each), Medusa mirrors (2310, 6300), grates,
  portcullis, collapsing floor. Live sent nearly every 1000057 entry as a server dynel (recorded
  playfields: 19 of 21 in 1300, 10 of 10 in 4600), with its own sequential instances; we only
  send the jump pads, so these are missing in game.
- No lever, gate or plate is used in the recorded session, and no client data links levers to
  gates or describes the plate puzzles (Gauntlet has quest areas "Quest_PressurePlateRoom",
  "…Complete", mirror/maze/dodge/buzz/shooting/tiki rooms in RDB 1000055): that was server script.
- Client side: a usable SimpleDynel (CanUse: CARS check, not CanBeOpened) sends a
  `UseDynelCommand` (StopUse queues one with 0x22). Levers and gates have EMotionFX motion sets
  (1050004 520506 / 475033): `open`, `close`, `anim_0` (closed state), `anim_1` (open state).
  `UpdateSimpleDynelIIR_c` mode 0 = u8 hidden, closed, can-be-opened, usable (+0x338..0x33b,
  no animation); mode 1 = ID32. `SimpleDynelVisualChangeIIR_c` states: 0/1 particle effect
  (looping/one shot), 2 stop particle, 3 visual data, 4 collision (KD-tree) id, 7 fade in,
  8 fade out (u32), 9/10/11 visual data variants. `ScryScriptIIR_c` 0-2 start client script
  groups, 3/4 delete them.

## Missing game data

The archive lacks the RDB packs (playfield data type 1000049, many 1000141, …). Funcom's patch
server `http://update.lego.funcom.com/legoupm` still serves them (2026-10); `--upstream` makes the
HTTP service fetch and cache them (`rdb-cache/`), unwrapping `IOz1` = "IOz1" + u32 LE size + zlib.
The client stores fetched resources in its own `RDB/` folder.

### Encodings

- `ClientPlayerData_t` (`0x7c0000`): u32 (unknown, we send the account), map<u32, character>,
  map<u32, u32>. Character entry (`0xb49e80`): u32, u32, str, str, str, u32, u32, str,
  5×u32, 64-bit (`0x60bcb0`), u32, u32, str, u32, u32, str, u32, u32 — field meanings not yet
  mapped (UI properties include m_Index, m_ActiveTemplateID, m_FactionId, m_CreateCharacter,
  m_Level, m_CollectedMinifigures; `ASCharacterData`).
- `DimensionData` (`0x12ea940`): 8×u32, bool, u32, bool (UI: m_Id, m_Name, m_Language, m_Load,
  m_Rules).

## Client logging

`dbDebug.conf` in the client folder: `DebugLevel=N` logs messages of level ≥ N (lower = more;
the shipped value is 3). `DebugLevel=1` with `TypeMode=select` logs every RPC dispatch
("Dispatcher_t ... calling 'X'"), FlowControl state changes and GameClient states (very large
logs). Parser: `0x4818a0`.

## Client Lua hooks

`Data/startup.lua` runs at boot. Useful API: `flow.Login(u, p)`, `flow.CanEnterGame()`,
`flow.EnterGame()`, `flow.IsInGame()`, `flow.IsInPlay()`, `flow.IsDisconnected()`,
`DWEngine.StartCoroutine(fn)`, `debug.log(level, msg)`, `engine.GetLoginUsername()`.
See `Data/smoketest.lua` (Funcom's automated test) and `tools/client/startup.lua`.

## Tooling

See the README "Tools" table. When the client hits an assertion it opens a dialog that would
send a bug report to Funcom on close — kill the process instead (`pkill -x LMO.exe`).
`winedbg` (attach + `bt all`, see `logs/bt.txt` usage in history) gives backtraces of a stuck client. Highlights: `tools/test-run.sh` (with `LMO_AUTOPLAY=1`) for one
end-to-end cycle including a screenshot of the game window (`grim -T` works on hidden
workspaces); `tools/ghidra/decompile.sh` / `xrefs.sh` against the analyzed project.

- **Linux build with symbols**: the Linux client of the same version (1.5.557666, from
  lmo.files.aronwk.com) exports ~143k demangled symbols. `tools/re/linux_fn.sh -l REGEX` lists
  them; `tools/re/linux_fn.sh REGEX` disassembles with named call targets. Prefer it to the
  Windows exe for finding code; addresses differ.
- `tools/re/rdb.py` reads `RDB/RDBHashIndex.bin` (`types`, `ids T`, `find ID`, `get T ID`).
- `tools/client-keys.sh` types into the hidden client window (Hyprland `send_key_state`).
- `tools/re/pe.py`, `d.py <va> [bytes]` (disassemble with string annotations), `dispatch.py <string>`
  (find string xrefs and containing function), `vt.py <func>` (vtable containing a function).
  They need a Python venv with `pefile capstone` (and `protobuf` for `extract_protos.py`).
- `tools/re/extract_protos.py LMO.exe proto/` regenerates `proto/` from descriptors embedded in
  the exe.
- Receiver dispatch functions log `X::DispatchY() not possible, stream bad after '<arg>'`, which
  gives the argument names and read order; sender functions log their full C++ signature.
