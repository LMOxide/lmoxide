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
constants `lmo_net::methods::<interface>::<METHOD>`. Most interfaces use 32-bit hashes; some use
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
3 → drop (`0x7d56e0`). Builders live in `lmo_net::iir`; layouts are documented next to each
builder in `crates/lmo-server/src/game.rs`.

Pitfalls found:
- Reading from an empty `BinaryStream` yields an uninitialised value. Every blob/list the
  client reads needs at least its count (`PlayfieldFIIR_t` blob 2 empty → out-of-memory dialog).
- Playfield ID32 type is 51100 (key of the PlayfieldFactory map, `0x96cee0`).
  `PlayfieldProxyData.running_instance` is the key of the client's playfield map.
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

- `tools/re/pe.py`, `d.py <va> [bytes]` (disassemble with string annotations), `dispatch.py <string>`
  (find string xrefs and containing function), `vt.py <func>` (vtable containing a function).
  They need a Python venv with `pefile capstone` (and `protobuf` for `extract_protos.py`).
- `tools/re/extract_protos.py LMO.exe proto/` regenerates `proto/` from descriptors embedded in
  the exe.
- Receiver dispatch functions log `X::DispatchY() not possible, stream bad after '<arg>'`, which
  gives the argument names and read order; sender functions log their full C++ signature.
