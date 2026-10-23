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
| 15 | **next**: the full update — IIR objects (`CoreFullPlayerCharacterIIR`, `CoreCharacterFullUpdateIIR`, `n3PlayfieldFullUpdateIIR_t`, `CharInPlayIIR_t`, …), presumably via `GameserverToClientRPC::PassBlob` (dispatch `0x7b6980`) | |

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

See the README "Tools" table. Highlights: `tools/test-run.sh` (with `LMO_AUTOPLAY=1`) for one
end-to-end cycle including a screenshot of the game window (`grim -T` works on hidden
workspaces); `tools/ghidra/decompile.sh` / `xrefs.sh` against the analyzed project.

- `tools/re/pe.py`, `d.py <va> [bytes]` (disassemble with string annotations), `dispatch.py <string>`
  (find string xrefs and containing function), `vt.py <func>` (vtable containing a function).
  They need a Python venv with `pefile capstone` (and `protobuf` for `extract_protos.py`).
- `tools/re/extract_protos.py LMO.exe proto/` regenerates `proto/` from descriptors embedded in
  the exe.
- Receiver dispatch functions log `X::DispatchY() not possible, stream bad after '<arg>'`, which
  gives the argument names and read order; sender functions log their full C++ signature.
